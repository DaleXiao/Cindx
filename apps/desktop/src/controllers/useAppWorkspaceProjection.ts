import { useMemo } from "react";
import type {
  AgentState,
  AgentTraceState,
  ChatMessageView,
  ProjectSessionState
} from "../tauri";
import type { WorkspaceView } from "../components/Sidebar";
import { normalizedSessionEffort } from "../appShellModel";
import {
  SESSION_STATE_CACHE_LIMIT,
  messagesWithOptimisticUserMessages
} from "../sessionRuntimeModel";

type AppWorkspaceProjectionInput = {
  activeView: WorkspaceView;
  agentState: AgentState | null;
  agentTraceState: AgentTraceState | null;
  busySessionIds: Set<string>;
  optimisticUserMessages: Map<string, ChatMessageView[]>;
  optimisticUserMessageRevision: number;
  projectSessionState: ProjectSessionState | null;
  selectedTraceStepId: string | null;
  sidebarQuery: string;
};

export function useAppWorkspaceProjection({
  activeView,
  agentState,
  agentTraceState,
  busySessionIds,
  optimisticUserMessages,
  optimisticUserMessageRevision,
  projectSessionState,
  selectedTraceStepId,
  sidebarQuery
}: AppWorkspaceProjectionInput) {
  const activeProject = useMemo(
    () => projectSessionState?.projects.find((project) => project.active) ?? null,
    [projectSessionState?.projects]
  );
  const activeSession = useMemo(
    () => projectSessionState?.sessions.find((session) => session.active) ?? null,
    [projectSessionState?.sessions]
  );
  const activeAgentState =
    activeSession && agentState?.sessionId === activeSession.id ? agentState : null;
  const activeSessionBusy = Boolean(activeSession && busySessionIds.has(activeSession.id));

  const { archivedSessions, projects, sessions } = useMemo(() => {
    const normalizedSidebarQuery = sidebarQuery.trim().toLowerCase();
    const allSessions = projectSessionState?.sessions ?? [];
    const sessions = allSessions
      .filter((session) => {
        if (session.archived) return false;
        if (normalizedSidebarQuery) {
          return `${session.name} ${session.detail}`
            .toLowerCase()
            .includes(normalizedSidebarQuery);
        }
        return !activeProject || session.projectId === activeProject.id;
      })
      .map((session) =>
        busySessionIds.has(session.id)
          ? { ...session, status: "Working", activity: "working" as const }
          : session
      );
    const archivedSessions = allSessions
      .filter((session) => session.archived)
      .sort((left, right) => (right.archivedAtMs ?? 0) - (left.archivedAtMs ?? 0));
    const projects = (projectSessionState?.projects ?? []).filter((project) => {
      if (!normalizedSidebarQuery) return true;
      const projectMatches = `${project.name} ${project.detail} ${project.root}`
        .toLowerCase()
        .includes(normalizedSidebarQuery);
      const matchingSessionExists = allSessions.some(
        (session) =>
          !session.archived &&
          session.projectId === project.id &&
          `${session.name} ${session.detail}`
            .toLowerCase()
            .includes(normalizedSidebarQuery)
      );
      return projectMatches || matchingSessionExists;
    });
    return { archivedSessions, projects, sessions };
  }, [activeProject, busySessionIds, projectSessionState, sidebarQuery]);

  const traceTurns = useMemo(
    () => (activeView === "timeline" ? agentTraceState?.turns ?? [] : []),
    [activeView, agentTraceState?.turns]
  );
  const traceSteps = useMemo(
    () => traceTurns.flatMap((turn) => turn.steps),
    [traceTurns]
  );
  const activeSessionTraceSteps = useMemo(
    () =>
      activeSession && agentTraceState?.sessionId === activeSession.id ? traceSteps : [],
    [activeSession, agentTraceState?.sessionId, traceSteps]
  );
  const visibleAgentMessages = useMemo(
    () =>
      activeView === "timeline"
        ? messagesWithOptimisticUserMessages(
            activeAgentState?.messages ?? [],
            activeSession ? optimisticUserMessages.get(activeSession.id) : undefined
          )
        : [],
    [
      activeAgentState?.messages,
      activeSession,
      activeView,
      optimisticUserMessageRevision,
      optimisticUserMessages
    ]
  );
  const selectedTraceStep = useMemo(
    () => traceSteps.find((step) => step.id === selectedTraceStepId) ?? null,
    [selectedTraceStepId, traceSteps]
  );

  const sessionPrefetchKey = useMemo(() => {
    const allSessions = projectSessionState?.sessions ?? [];
    const projectSessions = projectSessionState?.activeProjectId
      ? allSessions.filter(
          (session) =>
            session.projectId === projectSessionState.activeProjectId && !session.archived
        )
      : [];
    const activeIndex = projectSessions.findIndex(
      (session) => session.id === projectSessionState?.activeSessionId
    );
    const prioritizedSessions =
      activeIndex >= 0
        ? [
            projectSessions[activeIndex - 1],
            projectSessions[activeIndex + 1],
            projectSessions[0],
            projectSessions[projectSessions.length - 1],
            ...projectSessions
          ]
        : projectSessions;
    const sessionPrefetchKey = [
      ...new Set(
        prioritizedSessions
          .filter((session): session is (typeof projectSessions)[number] => Boolean(session))
          .map((session) => session.id)
      )
    ]
      .filter((sessionId) => sessionId !== projectSessionState?.activeSessionId)
      .slice(0, SESSION_STATE_CACHE_LIMIT - 1)
      .join("|");
    return sessionPrefetchKey;
  }, [projectSessionState]);

  return {
    activeAgentState,
    activeProject,
    activeSession,
    activeSessionBusy,
    activeSessionTraceSteps,
    agentEffort: normalizedSessionEffort(activeSession?.effort),
    archivedSessions,
    projects,
    selectedTraceStep,
    sessionPrefetchKey,
    sessions,
    traceSteps,
    traceTurns,
    visibleAgentMessages
  };
}
