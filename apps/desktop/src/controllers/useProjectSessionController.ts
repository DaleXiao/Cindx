import { useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { WorkspaceView } from "../appShellModel";
import {
  archiveSession,
  createProject,
  createSession,
  deleteProject,
  deleteSession,
  forkSession,
  getProjectSessionState,
  getRuntimeStatus,
  pickWorkspaceFolder,
  renameProject,
  renameSession,
  restoreSession,
  saveWorkspaceRoot,
  selectProject,
  selectSession
} from "../tauri";
import type {
  AgentState,
  ContextState,
  ProjectSessionState,
  RuntimeStatus
} from "../tauri";
import {
  agentStateUnchanged,
  mergeAgentStateSnapshot,
  projectReadSessionResult
} from "../sessionRuntimeModel";
import type { useSessionRuntimeController } from "./useSessionRuntimeController";

type SessionRuntime = ReturnType<typeof useSessionRuntimeController>;

type ProjectSessionControllerInput = {
  sessionRuntime: SessionRuntime;
  activeAgentState: AgentState | null;
  activeProject: ProjectSessionState["projects"][number] | null;
  activeView: WorkspaceView;
  enqueueProjectSessionSelection: (
    operation: () => Promise<ProjectSessionState>
  ) => Promise<ProjectSessionState>;
  forgetAttachments: (sessionIds: string[]) => void;
  forgetDrafts: (sessionIds: string[]) => void;
  newProjectName: string;
  refreshWorkspaceKnowledge: (shouldApply?: () => boolean) => Promise<void>;
  setComposerError: Dispatch<SetStateAction<string | null>>;
  setContextState: Dispatch<SetStateAction<ContextState | null>>;
  setNewProjectName: (name: string) => void;
  setProjectCreateOpen: Dispatch<SetStateAction<boolean>>;
  showSettingsSaved: () => void;
  showTimelineView: () => void;
};

export function useProjectSessionController({
  sessionRuntime,
  activeAgentState,
  activeProject,
  activeView,
  enqueueProjectSessionSelection,
  forgetAttachments,
  forgetDrafts,
  newProjectName,
  refreshWorkspaceKnowledge,
  setComposerError,
  setContextState,
  setNewProjectName,
  setProjectCreateOpen,
  showSettingsSaved,
  showTimelineView
}: ProjectSessionControllerInput) {
  const {
    activeSessionIdRef,
    agentStateRevisionsRef,
    acknowledgeOptimisticUserMessage,
    acknowledgeSessionResult,
    applySelectedSessionAgentState,
    busySessionIds,
    preserveOptimisticQueuedMessages,
    projectSessionState,
    requestSessionAgentState,
    restoreCachedSessionState,
    sessionLifecycleRefreshRef,
    sessionRuntimeCache,
    sessionSelectionRequestRef,
    setAgentState,
    setAgentTraceState,
    setBusySessionIds,
    setProjectSessionState,
    setSelectedThreadItem,
    setSelectedTraceStepId,
    setSessionLoadingId,
    setStreamResetVersion,
    updateSessionStatus
  } = sessionRuntime;
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspacePickerBusy, setWorkspacePickerBusy] = useState(false);
  const [projectSessionBusy, setProjectSessionBusy] = useState(false);
  const sessionRefreshRequestRef = useRef(0);

  async function handleSaveWorkspace() {
    const nextPath = workspaceDraft.trim();
    if (!nextPath) return;
    setWorkspaceBusy(true);
    setComposerError(null);
    try {
      const next = await saveWorkspaceRoot(nextPath);
      setRuntime(next);
      setWorkspaceDraft(next.workspaceRoot);
      setProjectSessionState(await getProjectSessionState());
      await refreshWorkspaceKnowledge();
      showSettingsSaved();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handlePickWorkspace() {
    if (workspacePickerBusy) return;
    setWorkspacePickerBusy(true);
    setComposerError(null);
    try {
      const nextPath = await pickWorkspaceFolder(workspaceDraft.trim() || undefined);
      if (nextPath) setWorkspaceDraft(nextPath);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setWorkspacePickerBusy(false);
    }
  }

  async function refreshWorkspaceAfterProjectSession(
    nextState: ProjectSessionState,
    prefetchedAgentState?: { sessionId: string; request: Promise<AgentState> }
  ) {
    const refreshRequest = ++sessionRefreshRequestRef.current;
    const previousSessionId = activeSessionIdRef.current;
    const sessionId = nextState.activeSessionId || null;
    const nextProject = nextState.projects.find((project) => project.id === nextState.activeProjectId);
    const nextWorkspaceRoot = nextProject?.root ?? "";
    const currentWorkspaceRoot = runtime?.workspaceRoot ?? activeProject?.root ?? "";
    const workspaceChanged = Boolean(
      nextWorkspaceRoot && nextWorkspaceRoot !== currentWorkspaceRoot
    );
    const isCurrentRequest = () =>
      sessionRefreshRequestRef.current === refreshRequest &&
      activeSessionIdRef.current === sessionId;
    const reportBackgroundError = (error: unknown) => {
      if (!isCurrentRequest()) return;
      setSessionLoadingId(null);
      setComposerError(error instanceof Error ? error.message : String(error));
    };
    const refreshWorkspaceScopedState = () => {
      void getRuntimeStatus()
        .then((nextRuntime) => {
          if (!isCurrentRequest()) return;
          setRuntime(nextRuntime);
          setWorkspaceDraft(nextRuntime.workspaceRoot);
        })
        .catch(reportBackgroundError);
      void refreshWorkspaceKnowledge(isCurrentRequest).catch(reportBackgroundError);
    };

    activeSessionIdRef.current = sessionId;
    if (sessionId) acknowledgeSessionResult(sessionId);
    setProjectSessionState(nextState);
    setComposerError(nextState.lastError);
    setSelectedThreadItem(null);
    setStreamResetVersion((version) => version + 1);
    if (nextWorkspaceRoot) {
      setWorkspaceDraft(nextWorkspaceRoot);
      setRuntime((current) =>
        current ? { ...current, workspaceRoot: nextWorkspaceRoot } : current
      );
    }
    if (previousSessionId !== sessionId) {
      if (sessionId) restoreCachedSessionState(sessionId);
      else {
        setAgentState(null);
        setAgentTraceState(null);
        setContextState(null);
      }
      setSelectedTraceStepId(null);
    }
    if (!sessionId) {
      setSessionLoadingId(null);
      if (workspaceChanged) refreshWorkspaceScopedState();
      return;
    }

    let nextAgentState: AgentState;
    try {
      nextAgentState = await (
        prefetchedAgentState?.sessionId === sessionId
          ? prefetchedAgentState.request
          : requestSessionAgentState(sessionId)
      );
    } catch (error) {
      reportBackgroundError(error);
      return;
    }
    if (!isCurrentRequest()) return;
    setSessionLoadingId(null);
    acknowledgeOptimisticUserMessage(sessionId, nextAgentState.messages);
    setAgentState((current) => {
      const merged = preserveOptimisticQueuedMessages(
        sessionId,
        mergeAgentStateSnapshot(current, nextAgentState)
      );
      return agentStateUnchanged(current, merged) ? current : merged;
    });
    updateSessionStatus(sessionId, nextAgentState.status, nextAgentState.canContinue);

    if (workspaceChanged) refreshWorkspaceScopedState();
  }

  function forgetDeletedSessions(sessionIds: string[]) {
    if (sessionIds.length === 0) return;
    const deleted = new Set(sessionIds);
    forgetDrafts(sessionIds);
    forgetAttachments(sessionIds);
    setBusySessionIds((current) => new Set([...current].filter((id) => !deleted.has(id))));
    deleted.forEach((sessionId) => {
      agentStateRevisionsRef.current.delete(sessionId);
      sessionRuntimeCache.forget(sessionId);
    });
  }

  async function handleCreateProject() {
    const name = newProjectName.trim();
    const root = workspaceDraft.trim() || runtime?.workspaceRoot || "";
    if (!name || !root) return;
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await createProject(name, root);
      setNewProjectName("");
      setProjectCreateOpen(false);
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleCreateSession() {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await createSession("New Session", activeProject?.id ?? undefined);
      await refreshWorkspaceAfterProjectSession(next);
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleSelectProject(projectId: string) {
    showTimelineView();
    if (projectId === projectSessionState?.activeProjectId) return;
    const selectionRequest = ++sessionSelectionRequestRef.current;
    const targetSession = projectSessionState?.sessions.find(
      (session) => session.projectId === projectId && !session.archived
    );
    activeSessionIdRef.current = targetSession?.id ?? null;
    if (targetSession) acknowledgeSessionResult(targetSession.id);
    setComposerError(null);
    setProjectSessionState((current) => {
      if (!current || !current.projects.some((project) => project.id === projectId)) {
        return current;
      }
      const nextSession =
        current.sessions.find(
          (session) =>
            session.id === current.activeSessionId &&
            session.projectId === projectId &&
            !session.archived
        ) ??
        current.sessions.find(
          (session) => session.projectId === projectId && !session.archived
        );
      return {
        ...current,
        activeProjectId: projectId,
        activeSessionId: nextSession?.id ?? "",
        projects: current.projects.map((project) => ({
          ...project,
          active: project.id === projectId
        })),
        sessions: current.sessions.map((session) => ({
          ...session,
          active: session.id === nextSession?.id
        }))
      };
    });
    if (targetSession) restoreCachedSessionState(targetSession.id);
    else {
      setSessionLoadingId(null);
      setAgentState(null);
      setAgentTraceState(null);
      setContextState(null);
    }
    setSelectedTraceStepId(null);
    setSelectedThreadItem(null);
    setStreamResetVersion((version) => version + 1);
    const agentStateRequest = targetSession
      ? requestSessionAgentState(targetSession.id)
      : null;
    if (targetSession && agentStateRequest) {
      applySelectedSessionAgentState(targetSession.id, selectionRequest, agentStateRequest);
    }
    try {
      const next = await enqueueProjectSessionSelection(() => selectProject(projectId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(
        next,
        targetSession && agentStateRequest
          ? { sessionId: targetSession.id, request: agentStateRequest }
          : undefined
      );
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    }
  }

  async function handleSelectSession(sessionId: string) {
    const leavingTimeline = activeView === "timeline";
    showTimelineView();
    if (sessionId === activeSessionIdRef.current) {
      if (!leavingTimeline) void acknowledgeSessionResult(sessionId);
      return;
    }
    const selectionRequest = ++sessionSelectionRequestRef.current;
    const previousSessionId = activeSessionIdRef.current;
    const previousReadSequence =
      activeAgentState?.sessionId === previousSessionId
        ? activeAgentState.latestSequence
        : projectSessionState?.sessions.find((session) => session.id === previousSessionId)
            ?.latestSequence;
    if (leavingTimeline && previousSessionId) {
      sessionLifecycleRefreshRef.current += 1;
      setProjectSessionState((current) =>
        current
          ? projectReadSessionResult(current, previousSessionId, previousReadSequence)
          : current
      );
      void acknowledgeSessionResult(previousSessionId, previousReadSequence);
    }
    activeSessionIdRef.current = sessionId;
    void acknowledgeSessionResult(sessionId);
    setComposerError(null);
    setProjectSessionState((current) => {
      if (!current) return current;
      const target = current.sessions.find((session) => session.id === sessionId);
      if (!target) return current;
      return {
        ...current,
        activeProjectId: target.projectId,
        activeSessionId: sessionId,
        projects: current.projects.map((project) => ({
          ...project,
          active: project.id === target.projectId
        })),
        sessions: current.sessions.map((session) => ({
          ...session,
          active: session.id === sessionId
        }))
      };
    });
    restoreCachedSessionState(sessionId);
    setSelectedTraceStepId(null);
    setSelectedThreadItem(null);
    setStreamResetVersion((version) => version + 1);
    const agentStateRequest = requestSessionAgentState(sessionId);
    applySelectedSessionAgentState(sessionId, selectionRequest, agentStateRequest);
    try {
      const next = await enqueueProjectSessionSelection(() => selectSession(sessionId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      const nextWithReadSession =
        leavingTimeline && previousSessionId
          ? projectReadSessionResult(next, previousSessionId, previousReadSequence)
          : next;
      await refreshWorkspaceAfterProjectSession(nextWithReadSession, {
        sessionId,
        request: agentStateRequest
      });
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    }
  }

  async function handleOpenScheduledSession(sessionId: string) {
    if (sessionId !== activeSessionIdRef.current) {
      await handleSelectSession(sessionId);
      return;
    }
    const selectionRequest = ++sessionSelectionRequestRef.current;
    showTimelineView();
    setSessionLoadingId(sessionId);
    setComposerError(null);
    try {
      const next = await enqueueProjectSessionSelection(() => selectSession(sessionId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setSessionLoadingId(null);
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    }
  }

  async function handleForkSession(sessionId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await forkSession(sessionId));
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleRenameSession(sessionId: string, name: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await renameSession(sessionId, name));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleRenameProject(projectId: string, name: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await renameProject(projectId, name));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleDeleteProject(projectId: string) {
    const sessionIds = (projectSessionState?.sessions ?? [])
      .filter((session) => session.projectId === projectId)
      .map((session) => session.id);
    if (
      sessionIds.some(
        (sessionId) =>
          busySessionIds.has(sessionId) ||
          projectSessionState?.sessions.find((session) => session.id === sessionId)
            ?.attentionReason === "permission"
      )
    ) {
      setComposerError("Stop the running sessions before deleting this project.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await deleteProject(projectId);
      forgetDeletedSessions(sessionIds);
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleArchiveSession(sessionId: string) {
    if (
      busySessionIds.has(sessionId) ||
      projectSessionState?.sessions.find((session) => session.id === sessionId)
        ?.attentionReason === "permission"
    ) {
      setComposerError("Stop the running session before archiving it.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await archiveSession(sessionId);
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleRestoreSession(sessionId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await restoreSession(sessionId);
      setProjectSessionState(next);
      setComposerError(next.lastError);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleDeleteSession(sessionId: string) {
    if (
      busySessionIds.has(sessionId) ||
      projectSessionState?.sessions.find((session) => session.id === sessionId)
        ?.attentionReason === "permission"
    ) {
      setComposerError("Stop the running session before deleting it.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await deleteSession(sessionId);
      forgetDeletedSessions([sessionId]);
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  function completeBulkDelete(ids: string[], next: ProjectSessionState | null, error: string | null) {
    forgetDeletedSessions(ids);
    if (error) setComposerError(error);
    if (next) void refreshWorkspaceAfterProjectSession(next);
  }

  return {
    completeBulkDelete,
    handleArchiveSession,
    handleCreateProject,
    handleCreateSession,
    handleDeleteProject,
    handleDeleteSession,
    handleForkSession,
    handleOpenScheduledSession,
    handlePickWorkspace,
    handleRenameProject,
    handleRenameSession,
    handleRestoreSession,
    handleSelectProject,
    handleSelectSession,
    handleSaveWorkspace,
    projectSessionBusy,
    refreshWorkspaceAfterProjectSession,
    runtime,
    setRuntime,
    setWorkspaceDraft,
    workspaceBusy,
    workspaceDraft,
    workspacePickerBusy
  };
}
