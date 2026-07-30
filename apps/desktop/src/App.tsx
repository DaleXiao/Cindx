import {
  lazy,
  startTransition,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties
} from "react";
import { CheckCircle2 } from "lucide-react";
import { Inspector, type InspectorTab } from "./components/Inspector";
import { Composer } from "./components/Composer";
import { QueuedMessages } from "./components/QueuedMessages";
import { Sidebar, type WorkspaceView } from "./components/Sidebar";
import type { SettingsCategory } from "./components/SettingsPage";
import { WorkspaceChrome } from "./components/WorkspaceChrome";
import {
  BACKGROUND_AGENT_POLL_INTERVAL_MS,
  FOREGROUND_AGENT_POLL_INTERVAL_MS,
  loadDebugAlwaysVisible,
  queuedMessageClientId
} from "./appShellModel";
import { optimisticRunBudgetPatch } from "./agentRunBudgetModel";
import { useAppWorkspaceProjection } from "./controllers/useAppWorkspaceProjection";
import { useComposerAttachments } from "./controllers/useComposerAttachments";
import { useComposerDrafts } from "./controllers/useComposerDrafts";
import { usePreferencesController } from "./controllers/usePreferencesController";
import { useProviderSettingsController } from "./controllers/useProviderSettingsController";
import { useIntegrationSettingsController } from "./controllers/useIntegrationSettingsController";
import { useKnowledgeToolingController } from "./controllers/useKnowledgeToolingController";
import { useLatestAsyncSelection } from "./controllers/useLatestAsyncSelection";
import { usePermissionReviewController } from "./controllers/usePermissionReviewController";
import { useSidebarResize } from "./controllers/useSidebarResize";
import {
  LiveSessionThread,
  type SessionThreadSelection
} from "./components/SessionThread";
import {
  AgentState,
  AgentEffort,
  AgentTraceState,
  ChatMessageView,
  acknowledgeSessionActivity,
  archiveSession,
  cancelAgentTask,
  createProject,
  createSession,
  deleteQueuedAgentMessage,
  deleteProject,
  deleteSession,
  exportAgentTraceJsonl,
  getAgentState,
  getAgentStateDelta,
  getAgentHistoryPage,
  getAgentStateRevision,
  getAgentTraceState,
  getContextState,
  getProjectSessionState,
  getRuntimeStatus,
  editQueuedAgentMessage,
  forkSession,
  PermissionReviewItem,
  ProjectSessionState,
  QueuedAgentMessage,
  QueuedAgentMessageActionReceipt,
  QueuedAgentMessageReceipt,
  revealArtifact,
  revealMainWindow,
  setSidebarMaterialWidth,
  resolveAgentPermission,
  resolvePermission,
  RuntimeStatus,
  retryAgentTask,
  renameProject,
  renameSession,
  restoreSession,
  runAgentTask,
  runNextQueuedAgentMessage,
  pickWorkspaceFolder,
  saveWorkspaceRoot,
  selectProject,
  selectSession,
  setSessionEffort,
  steerQueuedAgentMessage,
  subscribeToSessionTitleUpdates,
  queueAgentMessage
} from "./tauri";
import {
  SessionRuntimeCache,
  agentStateUnchanged,
  agentTraceUnchanged,
  committedSteerReconciliation,
  committedSteerUserMessage,
  containsOptimisticUserMessage,
  latestTraceStep,
  mergeAcknowledgedSessionActivity,
  mergeAgentStateDelta,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
  mergeSequencedItems,
  projectReadSessionResult
} from "./sessionRuntimeModel";

const SIDEBAR_MATERIAL_HIDE_DELAY_MS = 220;

const ScheduleView = lazy(() =>
  import("./components/ScheduleView").then((module) => ({
    default: module.ScheduleView
  }))
);

const SettingsPage = lazy(() =>
  import("./components/SettingsPage").then((module) => ({
    default: module.SettingsPage
  }))
);

export function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [activeView, setActiveView] = useState<WorkspaceView>("timeline");
  const [selectedScheduleId, setSelectedScheduleId] = useState<string | null>(null);
  const [workspaceViewBeforeSettings, setWorkspaceViewBeforeSettings] =
    useState<Exclude<WorkspaceView, "settings">>("timeline");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const {
    beginResize: beginSidebarResize,
    handleResizeKeyDown: handleSidebarResizeKeyDown,
    resizing: sidebarResizing,
    width: sidebarWidth
  } = useSidebarResize();
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("runtime");
  const {
    appearanceMode,
    flushPersonalization,
    handleAppearanceModeChange,
    handleSavePersonalization,
    loadPersonalization,
    personalizationBusy,
    personalizationDraft,
    personalizationError,
    settingsToast,
    showSettingsSaved,
    updatePersonalizationDraft
  } = usePreferencesController();
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("details");
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [inspectorOutputRequest, setInspectorOutputRequest] = useState<{
    sessionId: string;
    path: string;
    nonce: number;
  } | null>(null);
  const [inspectorOpenBeforeSettings, setInspectorOpenBeforeSettings] = useState(false);
  const [inspectorOpenBeforeSchedule, setInspectorOpenBeforeSchedule] = useState(false);
  const [inspectorWidth, setInspectorWidth] = useState(320);
  const [inspectorResizing, setInspectorResizing] = useState(false);
  const [debugAlwaysVisible, setDebugAlwaysVisible] = useState(loadDebugAlwaysVisible);
  const {
    activeReviews: activePermissionReviews,
    ignoreReview: handleIgnorePermissionReview,
    ignoredReviews: ignoredPermissionReviews,
    refresh: refreshPermissionReviews,
    restoreReview: handleRestorePermissionReview
  } = usePermissionReviewController(
    activeView === "settings" && settingsCategory === "permissions"
  );
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [sessionLoadingId, setSessionLoadingId] = useState<string | null>(null);
  const [agentTraceState, setAgentTraceState] = useState<AgentTraceState | null>(null);
  const [selectedThreadItem, setSelectedThreadItem] =
    useState<SessionThreadSelection | null>(null);
  const [selectedTraceStepId, setSelectedTraceStepId] = useState<string | null>(null);
  const [projectSessionState, setProjectSessionState] = useState<ProjectSessionState | null>(null);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [newProjectName, setNewProjectName] = useState("");
  const [projectCreateOpen, setProjectCreateOpen] = useState(false);
  const [sidebarSearchOpen, setSidebarSearchOpen] = useState(false);
  const [sidebarQuery, setSidebarQuery] = useState("");
  const [streamResetVersion, setStreamResetVersion] = useState(0);
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspacePickerBusy, setWorkspacePickerBusy] = useState(false);
  const [traceBusy, setTraceBusy] = useState(false);
  const [projectSessionBusy, setProjectSessionBusy] = useState(false);
  const [busySessionIds, setBusySessionIds] = useState<Set<string>>(() => new Set());
  const [queuedMessageBusyId, setQueuedMessageBusyId] = useState<string | null>(null);
  const [persistingQueuedMessageIds, setPersistingQueuedMessageIds] = useState<Set<string>>(
    () => new Set()
  );
  const activeSessionIdRef = useRef<string | null>(null);
  const agentStateRevisionsRef = useRef<
    Map<string, { eventCount: number; latestSequence: number; latestTimestampMs: number }>
  >(new Map());
  const [sessionRuntimeCache] = useState(() => new SessionRuntimeCache());
  const agentHistoryRequestsRef = useRef<Set<string>>(new Set());
  const [loadingOlderSessionId, setLoadingOlderSessionId] = useState<string | null>(null);
  const sessionSelectionRequestRef = useRef(0);
  const enqueueProjectSessionSelection = useLatestAsyncSelection<ProjectSessionState>();
  const sessionRefreshRequestRef = useRef(0);
  const sessionLifecycleRefreshRef = useRef(0);
  const optimisticUserMessagesRef = useRef<Map<string, ChatMessageView[]>>(new Map());
  const [optimisticUserMessageRevision, setOptimisticUserMessageRevision] = useState(0);
  const optimisticQueuedMessagesRef = useRef<Map<string, QueuedAgentMessage>>(new Map());
  const optimisticallyDeletedQueuedMessagesRef = useRef<Map<string, string>>(new Map());
  const steeredQueuedMessageIdsRef = useRef<Set<string>>(new Set());
  const queueDrainingSessionIdsRef = useRef<Set<string>>(new Set());
  const suppressQueueDrainSessionIdsRef = useRef<Set<string>>(new Set());
  const startupWindowRevealRequestedRef = useRef(false);
  const [composerError, setComposerError] = useState<string | null>(null);
  const reportComposerError = useCallback(
    (message: string | null) => setComposerError(message),
    []
  );
  const showInspector = useCallback((tab: InspectorTab) => {
    setInspectorTab(tab);
    setInspectorOpen(true);
  }, []);
  const {
    canUseConfiguredKey,
    collaborationModelCount,
    handleLoadProviderModels,
    handlePromptEvolutionToggle,
    handleSaveProviderConfig,
    imageEndpointValidation,
    loadProviderState,
    phase4,
    providerBusy,
    providerDraft,
    providerModelOptions,
    providerModels,
    providerModelsBusy,
    providerModelsError,
    providerModelsRefreshTurn,
    setProviderDraft,
    voiceConfigured,
    voiceTransport
  } = useProviderSettingsController({
    reportError: reportComposerError,
    showSaved: showSettingsSaved
  });
  const {
    handleAddMcpServer,
    handleInstallSkillPackage,
    handleInstallSkillUrl,
    handleMcpPolicy,
    handleRefreshMcpServer,
    handleRefreshSkills,
    handleRemoveMcpServer,
    handleSaveSidecars,
    handleSaveWebSearch,
    handleSkillPreference,
    loadIntegrationState,
    mcpBusy,
    mcpDraft,
    mcpState,
    setMcpDraft,
    setSidecarDraft,
    setSkillUrl,
    setWebSearchDraft,
    sidecarBusy,
    sidecarDraft,
    sidecarState,
    skillBusy,
    skillInstallError,
    skillPackageInputRef,
    skillRefreshTurn,
    skillState,
    skillUrl,
    webSearchBusy,
    webSearchConfig,
    webSearchDraft,
    webSearchError
  } = useIntegrationSettingsController({
    reportError: reportComposerError,
    showSaved: showSettingsSaved
  });
  const {
    browserApprovals,
    browserBusy,
    browserObservations,
    browserTarget,
    browserText,
    browserUrl,
    contextBusy,
    contextCheckpoint,
    contextState,
    handleAnswerWithRag,
    handleCompactContext,
    handleIndexRag,
    handleResolveBrowserPermission,
    handleResolveToolPermission,
    handleRunBrowserTool,
    handleRunTool,
    handleSearchRag,
    knowledgeError,
    knowledgeGraphOpen,
    loadKnowledgeState,
    phase5,
    phase7,
    ragBusy,
    ragQuery,
    ragSources,
    ragStats,
    refreshWorkspaceKnowledge,
    selectedTool,
    selectedToolSpec,
    setBrowserTarget,
    setBrowserText,
    setBrowserUrl,
    setContextState,
    setKnowledgeGraphOpen,
    setRagQuery,
    setSelectedTool,
    setToolInput,
    toolApprovals,
    toolBusy,
    toolInput,
    toolResults
  } = useKnowledgeToolingController({
    reportError: reportComposerError,
    showInspector
  });

  function requestSessionAgentState(sessionId: string) {
    return sessionRuntimeCache.requestAgent(sessionId, () =>
      getAgentState(sessionId).then((next) => {
        agentStateRevisionsRef.current.set(sessionId, {
          eventCount: next.eventCount,
          latestSequence: next.latestSequence,
          latestTimestampMs: 0
        });
        return next;
      })
    );
  }

  function applySelectedSessionAgentState(
    sessionId: string,
    selectionRequest: number,
    request: Promise<AgentState>
  ) {
    void request
      .then((nextAgentState) => {
        if (
          selectionRequest !== sessionSelectionRequestRef.current ||
          activeSessionIdRef.current !== sessionId
        ) {
          return;
        }
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
      })
      .catch((error) => {
        if (
          selectionRequest === sessionSelectionRequestRef.current &&
          activeSessionIdRef.current === sessionId
        ) {
          setSessionLoadingId(null);
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      });
  }

  function restoreCachedSessionState(sessionId: string) {
    const cached = sessionRuntimeCache.read(sessionId);
    setSessionLoadingId(cached.agent ? null : sessionId);
    setAgentState(cached.agent);
    setAgentTraceState(cached.trace);
    setContextState(cached.context);
  }

  const handleThreadSelection = useCallback((selection: SessionThreadSelection) => {
    setSelectedThreadItem(selection);
    setSelectedTraceStepId(null);
    setInspectorTab("details");
    setInspectorOpen(true);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | null = null;
    void subscribeToSessionTitleUpdates((sessionId) => {
      void getProjectSessionState()
        .then((nextState) => {
          if (disposed) return;
          const renamedSession = nextState.sessions.find((session) => session.id === sessionId);
          if (!renamedSession) return;
          setProjectSessionState((current) =>
            current
              ? {
                  ...current,
                  sessions: current.sessions.map((session) =>
                    session.id === sessionId ? renamedSession : session
                  )
                }
              : nextState
          );
          setAgentState((current) =>
            current?.sessionId === sessionId
              ? { ...current, sessionName: renamedSession.name }
              : current
          );
        })
        .catch(() => {});
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unsubscribe = unlisten;
    });
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    let frame: number | null = null;
    let timer: number | null = null;
    const width = activeView === "settings" || !sidebarOpen ? 0 : sidebarWidth;
    const updateMaterial = () => {
      frame = window.requestAnimationFrame(() => {
        void setSidebarMaterialWidth(width).catch(() => {});
      });
    };

    if (width === 0) {
      timer = window.setTimeout(updateMaterial, SIDEBAR_MATERIAL_HIDE_DELAY_MS);
    } else {
      updateMaterial();
    }

    return () => {
      if (timer !== null) window.clearTimeout(timer);
      if (frame !== null) window.cancelAnimationFrame(frame);
    };
  }, [activeView, sidebarOpen, sidebarWidth]);

  useEffect(() => {
    let disposed = false;
    let deferredLoadTimer: number | null = null;
    let deferredIdleCallback: number | null = null;

    const coreRequests = [
      getRuntimeStatus().then((state) => {
        setRuntime(state);
        setWorkspaceDraft(state.workspaceRoot);
      }),
      getProjectSessionState().then((state) => {
        setProjectSessionState(state);
        setSessionLoadingId(
          state.activeSessionId && !sessionRuntimeCache.hasAgent(state.activeSessionId)
            ? state.activeSessionId
            : null
        );
        setComposerError((current) => current ?? state.lastError);
      }),
      getAgentState().then((state) => {
        if (state.sessionId) {
          agentStateRevisionsRef.current.set(state.sessionId, {
            eventCount: state.eventCount,
            latestSequence: state.latestSequence,
            latestTimestampMs: 0
          });
          sessionRuntimeCache.rememberAgent(state.sessionId, state);
        }
        setAgentState(state);
        setSessionLoadingId(null);
        if (state.sessionId) {
          updateSessionStatus(state.sessionId, state.status, state.canContinue);
        }
        setComposerError((current) => current ?? state.lastError);
      }),
      loadPersonalization()
    ];

    const loadDeferredState = () => {
      if (disposed) return;
      loadIntegrationState();
      void refreshPermissionReviews().catch(() => {});
      void loadProviderState();
      loadKnowledgeState();
    };

    void Promise.allSettled(coreRequests).then(() => {
      if (disposed) return;
      if (typeof window.requestIdleCallback === "function") {
        deferredIdleCallback = window.requestIdleCallback(loadDeferredState, { timeout: 1000 });
      } else {
        deferredLoadTimer = window.setTimeout(loadDeferredState, 250);
      }
    });

    return () => {
      disposed = true;
      if (deferredLoadTimer !== null) window.clearTimeout(deferredLoadTimer);
      if (deferredIdleCallback !== null) window.cancelIdleCallback(deferredIdleCallback);
    };
  }, []);

  useEffect(() => {
    if (startupWindowRevealRequestedRef.current || !runtime || !projectSessionState) return;
    let disposed = false;
    let fontWaitTimer: number | null = null;
    const reveal = async () => {
      try {
        await Promise.race([
          document.fonts.ready,
          new Promise<void>((resolve) => {
            fontWaitTimer = window.setTimeout(resolve, 120);
          })
        ]);
      } catch {
        // A missing font should not keep the native window hidden.
      }
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
      if (disposed || startupWindowRevealRequestedRef.current) return;
      startupWindowRevealRequestedRef.current = true;
      await revealMainWindow().catch(() => {});
    };
    void reveal();
    return () => {
      disposed = true;
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
    };
  }, [projectSessionState, runtime]);

  const statusText = useMemo(() => {
    if (!runtime) return "Connecting";
    if (runtime.kernelStatus === "kernel bridge online") return "Ready";
    return runtime.kernelStatus;
  }, [runtime]);

  const {
    activeAgentState,
    activeProject,
    activeSession,
    activeSessionBusy,
    activeSessionTraceSteps,
    agentEffort,
    archivedSessions,
    projects,
    selectedTraceStep,
    sessionPrefetchKey,
    sessions,
    visibleAgentMessages
  } = useAppWorkspaceProjection({
    activeView,
    agentState,
    agentTraceState,
    busySessionIds,
    optimisticUserMessages: optimisticUserMessagesRef.current,
    optimisticUserMessageRevision,
    projectSessionState,
    selectedTraceStepId,
    sidebarQuery
  });
  const {
    attachments: composerAttachments,
    busy: attachmentBusy,
    clear: clearAttachments,
    forget: forgetAttachments,
    pick: handlePickAttachments,
    remove: handleRemoveAttachment,
    restoreIfEmpty: restoreAttachmentsIfEmpty
  } = useComposerAttachments({
    reportError: reportComposerError,
    sessionId: activeSession?.id ?? null
  });

  function preserveOptimisticQueuedMessages(sessionId: string, state: AgentState) {
    steeredQueuedMessageIdsRef.current.forEach((queueId) => {
      if (optimisticallyDeletedQueuedMessagesRef.current.get(queueId) !== sessionId) return;
      const resolution = committedSteerReconciliation(state, queueId);
      if (resolution === "pending") return;
      steeredQueuedMessageIdsRef.current.delete(queueId);
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      const optimistic = optimisticUserMessagesRef.current.get(sessionId);
      if (!optimistic) return;
      const next = optimistic.filter((message) => message.queueId !== queueId);
      if (next.length > 0) {
        optimisticUserMessagesRef.current.set(sessionId, next);
      } else {
        optimisticUserMessagesRef.current.delete(sessionId);
      }
    });
    let queuedMessages = state.queuedMessages;
    optimisticallyDeletedQueuedMessagesRef.current.forEach((targetSessionId, queueId) => {
      if (targetSessionId === sessionId) {
        queuedMessages = queuedMessages.filter((message) => message.id !== queueId);
      }
    });
    optimisticQueuedMessagesRef.current.forEach((message) => {
      if (message.sessionId === sessionId) {
        queuedMessages = mergeQueuedAgentMessage(queuedMessages, message);
      }
    });
    return queuedMessages === state.queuedMessages ? state : { ...state, queuedMessages };
  }

  const loadOlderAgentHistory = useCallback(async () => {
    const sessionId = activeAgentState?.sessionId;
    if (
      !activeAgentState ||
      !sessionId ||
      !activeAgentState.hasOlderHistory ||
      activeAgentState.oldestSequence <= 0 ||
      agentHistoryRequestsRef.current.has(sessionId)
    ) {
      return;
    }
    agentHistoryRequestsRef.current.add(sessionId);
    setLoadingOlderSessionId(sessionId);
    try {
      const page = await getAgentHistoryPage(
        sessionId,
        activeAgentState.oldestSequence
      );
      setAgentState((current) => {
        if (!current || current.sessionId !== page.sessionId) return current;
        return {
          ...current,
          oldestSequence: page.oldestSequence,
          hasOlderHistory: page.hasOlderHistory,
          timeline: mergeSequencedItems(page.timeline, current.timeline),
          messages: mergeSequencedItems(page.messages, current.messages)
        };
      });
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      agentHistoryRequestsRef.current.delete(sessionId);
      setLoadingOlderSessionId((current) => (current === sessionId ? null : current));
    }
  }, [activeAgentState?.hasOlderHistory, activeAgentState?.oldestSequence, activeAgentState?.sessionId]);

  useEffect(() => {
    activeSessionIdRef.current = activeSession?.id ?? null;
  }, [activeSession?.id]);

  const handleAgentStreamDone = useCallback(async (sessionId: string) => {
    if (activeSessionIdRef.current !== sessionId) return false;
    try {
      const next = await requestSessionAgentState(sessionId);
      if (activeSessionIdRef.current !== sessionId) return false;
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      setAgentState((current) => {
        const merged = preserveOptimisticQueuedMessages(
          sessionId,
          mergeAgentStateSnapshot(current, next)
        );
        return agentStateUnchanged(current, merged) ? current : merged;
      });
      updateSessionStatus(sessionId, next.status, next.canContinue);
      return next.status === "completed" && Boolean(next.latestAnswer?.trim());
    } catch (error) {
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
      return false;
    }
  }, []);

  useEffect(() => {
    if (!agentState?.sessionId) return;
    sessionRuntimeCache.rememberAgent(agentState.sessionId, agentState);
  }, [agentState, sessionRuntimeCache]);

  useEffect(() => {
    if (!agentTraceState?.sessionId) return;
    sessionRuntimeCache.rememberTrace(agentTraceState.sessionId, agentTraceState);
  }, [agentTraceState, sessionRuntimeCache]);

  useEffect(() => {
    if (!sessionPrefetchKey || activeSessionBusy) return;
    let disposed = false;
    let idleCallback: number | null = null;
    let fallbackTimer: number | null = null;
    const sessionIds = sessionPrefetchKey.split("|");
    const prefetch = async () => {
      if (disposed) return;
      const pending = sessionIds.filter(
        (sessionId) => !sessionRuntimeCache.hasAgent(sessionId)
      );
      let nextIndex = 0;
      const worker = async () => {
        while (!disposed && nextIndex < pending.length) {
          const sessionId = pending[nextIndex];
          nextIndex += 1;
          await requestSessionAgentState(sessionId).catch(() => null);
        }
      };
      await Promise.all([worker(), worker()]);
    };
    if (typeof window.requestIdleCallback === "function") {
      idleCallback = window.requestIdleCallback(() => void prefetch(), { timeout: 2_000 });
    } else {
      fallbackTimer = window.setTimeout(() => void prefetch(), 1_200);
    }
    return () => {
      disposed = true;
      if (idleCallback !== null) window.cancelIdleCallback(idleCallback);
      if (fallbackTimer !== null) window.clearTimeout(fallbackTimer);
    };
  }, [activeSessionBusy, sessionPrefetchKey, sessionRuntimeCache]);

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!sessionId || !activeSessionBusy) return;
    let disposed = false;
    let inFlight = false;
    let lastTraceRefreshAt = 0;
    let lastBackgroundRefreshAt = 0;
    const refresh = async () => {
      if (inFlight) return;
      const now = Date.now();
      if (
        document.visibilityState === "hidden" &&
        now - lastBackgroundRefreshAt < BACKGROUND_AGENT_POLL_INTERVAL_MS
      ) {
        return;
      }
      if (document.visibilityState === "hidden") lastBackgroundRefreshAt = now;
      inFlight = true;
      try {
        const refreshTrace =
          activeView === "timeline" && inspectorOpen && now - lastTraceRefreshAt >= 3_000;
        if (refreshTrace) lastTraceRefreshAt = now;
        const revision = await getAgentStateRevision(sessionId);
        const previousRevision = agentStateRevisionsRef.current.get(sessionId);
        const stateChanged =
          !previousRevision ||
          previousRevision.eventCount !== revision.eventCount ||
          previousRevision.latestSequence !== revision.latestSequence;
        const [nextDelta, nextTrace] = await Promise.all([
          stateChanged
            ? getAgentStateDelta(sessionId, previousRevision?.latestSequence ?? 0)
            : Promise.resolve(null),
          refreshTrace
            ? getAgentTraceState(sessionId).catch(() => null)
            : Promise.resolve(null)
        ]);
        if (!disposed && activeSessionIdRef.current === sessionId) {
          agentStateRevisionsRef.current.set(sessionId, revision);
          if (nextDelta) {
            acknowledgeOptimisticUserMessage(sessionId, nextDelta.state.messages);
            setAgentState((current) => {
              const next = preserveOptimisticQueuedMessages(
                sessionId,
                mergeAgentStateDelta(current, nextDelta)
              );
              return agentStateUnchanged(current, next) ? current : next;
            });
            updateSessionStatus(
              sessionId,
              nextDelta.state.status,
              nextDelta.state.canContinue
            );
          }
          if (nextTrace) {
            setAgentTraceState((current) =>
              agentTraceUnchanged(current, nextTrace) ? current : nextTrace
            );
          }
        }
      } catch (error) {
        if (!disposed) {
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      } finally {
        inFlight = false;
      }
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    void refresh();
    document.addEventListener("visibilitychange", handleVisibilityChange);
    const interval = window.setInterval(
      () => void refresh(),
      activeView === "timeline"
        ? FOREGROUND_AGENT_POLL_INTERVAL_MS
        : BACKGROUND_AGENT_POLL_INTERVAL_MS
    );
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      window.clearInterval(interval);
    };
  }, [activeSession?.id, activeSessionBusy, activeView, inspectorOpen]);

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!inspectorOpen || !sessionId || activeSessionBusy) return;
    let disposed = false;
    void Promise.all([getAgentTraceState(sessionId), getContextState(sessionId)])
      .then(([nextTrace, nextContext]) => {
        if (disposed || activeSessionIdRef.current !== sessionId) return;
        sessionRuntimeCache.rememberTrace(sessionId, nextTrace);
        sessionRuntimeCache.rememberContext(sessionId, nextContext);
        startTransition(() => {
          setAgentTraceState((current) =>
            agentTraceUnchanged(current, nextTrace) ? current : nextTrace
          );
          setContextState(nextContext);
        });
        setSelectedTraceStepId(latestTraceStep(nextTrace.turns)?.id ?? null);
      })
      .catch((error) => {
        if (!disposed) setComposerError(error instanceof Error ? error.message : String(error));
      });
    return () => {
      disposed = true;
    };
  }, [activeSession?.id, activeSessionBusy, inspectorOpen, sessionRuntimeCache]);

  const {
    value: composerDraft,
    focusRequest: composerFocusRequest,
    setActiveDraft: setActiveComposerDraft,
    appendDraftForSession: appendComposerDraftForSession,
    editActiveDraft: handleThreadMessageEdit,
    restoreDraftIfEmpty,
    forgetDrafts
  } = useComposerDrafts(activeSession?.id ?? null, activeSessionIdRef);

  const agentApprovals = activeAgentState?.pendingApprovals ?? [];
  const agentCanCancel = Boolean(activeAgentState?.canCancel || activeSessionBusy);
  const agentCanRetry = Boolean(activeAgentState?.canRetry);
  const agentCanContinue = Boolean(activeAgentState?.canContinue);
  const agentWorking = Boolean(activeSessionBusy || activeAgentState?.status === "running");
  function markSessionBusy(sessionId: string, busy: boolean) {
    setBusySessionIds((current) => {
      const next = new Set(current);
      if (busy) next.add(sessionId);
      else next.delete(sessionId);
      return next;
    });
  }

  function acknowledgeSessionResult(sessionId: string, throughSequence?: number) {
    return acknowledgeSessionActivity(sessionId, throughSequence)
      .then((nextState) => {
        const acknowledged = nextState.sessions.find((session) => session.id === sessionId);
        if (!acknowledged) return;
        setProjectSessionState((current) =>
          current
            ? {
                ...current,
                sessions: current.sessions.map((session) =>
                  session.id === sessionId
                    ? mergeAcknowledgedSessionActivity(session, acknowledged)
                    : session
                )
              }
            : current
        );
      })
      .catch(() => {});
  }

  function updateSessionStatus(
    _sessionId: string,
    status: AgentState["status"],
    _canContinue = false
  ) {
    if (status === "running") return;
    const refreshRequest = ++sessionLifecycleRefreshRef.current;
    void getProjectSessionState()
      .then((nextState) => {
        if (sessionLifecycleRefreshRef.current === refreshRequest) {
          setProjectSessionState(nextState);
        }
      })
      .catch(() => {});
  }

  function acknowledgeOptimisticUserMessage(
    sessionId: string,
    messages: ChatMessageView[]
  ) {
    const optimistic = optimisticUserMessagesRef.current.get(sessionId);
    if (!optimistic) return;
    const pending = optimistic.filter(
      (message) => !containsOptimisticUserMessage(messages, message)
    );
    if (pending.length === optimistic.length) return;
    if (pending.length > 0) {
      optimisticUserMessagesRef.current.set(sessionId, pending);
    } else {
      optimisticUserMessagesRef.current.delete(sessionId);
    }
    setOptimisticUserMessageRevision((revision) => revision + 1);
  }

  function addOptimisticUserMessage(sessionId: string, message: ChatMessageView) {
    const current = optimisticUserMessagesRef.current.get(sessionId) ?? [];
    optimisticUserMessagesRef.current.set(sessionId, [...current, message]);
    setOptimisticUserMessageRevision((revision) => revision + 1);
  }

  async function refreshAgentTrace(
    selectLatest = false,
    sessionId = activeSessionIdRef.current
  ) {
    const next = await getAgentTraceState(sessionId);
    if (sessionId === activeSessionIdRef.current) {
      setAgentTraceState(next);
      if (selectLatest) {
        const latestStep = latestTraceStep(next.turns);
        setSelectedTraceStepId(latestStep?.id ?? null);
      }
      setComposerError((current) => current ?? next.lastError);
    }
    return next;
  }

  async function handleResolvePermissionReview(
    review: PermissionReviewItem,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    setPermissionBusy(true);
    try {
      if (review.source === "agent") {
        const sessionId = review.sessionId ?? activeSession?.id;
        if (!sessionId) throw new Error("The related session is no longer available.");
        await handleResolveAgentPermission(review.requestId, decision, sessionId);
      } else if (review.source === "tool") {
        await handleResolveToolPermission(review.requestId, decision);
      } else if (review.source === "browser") {
        await handleResolveBrowserPermission(review.requestId, decision);
      } else {
        await resolvePermission(review.requestId, decision);
      }
      handleRestorePermissionReview(review.requestId);
    } finally {
      await refreshPermissionReviews().catch(() => {});
      setPermissionBusy(false);
    }
  }

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

  function showWorkspaceView(view: Exclude<WorkspaceView, "settings">) {
    const leavingSettings = activeView === "settings";
    const leavingSchedule = activeView === "schedule" && view === "timeline";
    const enteringSchedule = activeView !== "schedule" && view === "schedule";
    if (enteringSchedule) {
      if (activeView === "timeline") setInspectorOpenBeforeSchedule(inspectorOpen);
      setInspectorOpen(false);
    }
    setWorkspaceViewBeforeSettings(view);
    setActiveView(view);
    if (leavingSettings && view === "timeline") {
      setInspectorOpen(inspectorOpenBeforeSettings);
    } else if (leavingSchedule) {
      setInspectorOpen(inspectorOpenBeforeSchedule);
    }
  }

  function showTimelineView() {
    showWorkspaceView("timeline");
  }

  function handleWorkspaceViewChange(view: WorkspaceView) {
    if (view === "settings") {
      if (activeView === "settings") {
        showWorkspaceView(workspaceViewBeforeSettings);
        return;
      }
      setWorkspaceViewBeforeSettings(activeView);
      setInspectorOpenBeforeSettings(
        activeView === "schedule" ? inspectorOpenBeforeSchedule : inspectorOpen
      );
      setActiveView("settings");
      setInspectorOpen(false);
      return;
    }
    showWorkspaceView(view);
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

  async function handleSessionEffortChange(effort: AgentEffort) {
    const sessionId = activeSession?.id;
    if (!sessionId || effort === agentEffort) return;
    const previousEffort = agentEffort;
    setProjectSessionState((current) =>
      current
        ? {
            ...current,
            sessions: current.sessions.map((session) =>
              session.id === sessionId ? { ...session, effort } : session
            )
          }
        : current
    );
    try {
      const next = await setSessionEffort(sessionId, effort);
      const persisted = next.sessions.find((session) => session.id === sessionId)?.effort ?? effort;
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === sessionId ? { ...session, effort: persisted } : session
              )
            }
          : next
      );
    } catch (error) {
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === sessionId ? { ...session, effort: previousEffort } : session
              )
            }
          : current
      );
      setComposerError(error instanceof Error ? error.message : String(error));
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


  function applyAgentStateForSession(sessionId: string, next: AgentState) {
    const effectiveNext = preserveOptimisticQueuedMessages(sessionId, next);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: effectiveNext.eventCount,
      latestSequence: effectiveNext.latestSequence,
      latestTimestampMs: 0
    });
    sessionRuntimeCache.rememberAgent(sessionId, effectiveNext);
    acknowledgeOptimisticUserMessage(sessionId, effectiveNext.messages);
    updateSessionStatus(sessionId, effectiveNext.status, effectiveNext.canContinue);
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => mergeAgentStateSnapshot(current, effectiveNext));
    }
  }

  function updateQueuedMessagesForSession(
    sessionId: string,
    update: (messages: QueuedAgentMessage[]) => QueuedAgentMessage[]
  ) {
    const updateState = (current: AgentState) => {
      const queuedMessages = update(current.queuedMessages);
      return queuedMessages === current.queuedMessages
        ? current
        : { ...current, queuedMessages };
    };
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    if (cached) {
      sessionRuntimeCache.rememberAgent(sessionId, updateState(cached));
    }
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = updateState(current);
        sessionRuntimeCache.rememberAgent(sessionId, next);
        return next;
      });
    }
  }

  function queuedMessageForSession(sessionId: string, queueId: string) {
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    const state = agentState?.sessionId === sessionId ? agentState : cached;
    return state?.queuedMessages.find((message) => message.id === queueId) ?? null;
  }

  function applyQueuedMessageReceiptForSession(
    sessionId: string,
    receipt: QueuedAgentMessageReceipt
  ) {
    const revision = agentStateRevisionsRef.current.get(sessionId);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: Math.max(revision?.eventCount ?? 0, receipt.eventCount),
      latestSequence: Math.max(revision?.latestSequence ?? 0, receipt.latestSequence),
      latestTimestampMs: Math.max(
        revision?.latestTimestampMs ?? 0,
        receipt.latestTimestampMs
      )
    });
    const mergeReceipt = (current: AgentState) => ({
      ...current,
      eventCount: Math.max(current.eventCount, receipt.eventCount),
      latestSequence: Math.max(current.latestSequence, receipt.latestSequence),
      queuedMessages: mergeQueuedAgentMessage(current.queuedMessages, receipt.message)
    });
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    if (cached) sessionRuntimeCache.rememberAgent(sessionId, mergeReceipt(cached));
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = mergeReceipt(current);
        sessionRuntimeCache.rememberAgent(sessionId, next);
        return next;
      });
    }
  }

  function applyQueuedMessageActionReceiptForSession(
    sessionId: string,
    receipt: QueuedAgentMessageActionReceipt
  ) {
    const revision = agentStateRevisionsRef.current.get(sessionId);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: Math.max(revision?.eventCount ?? 0, receipt.eventCount),
      latestSequence: Math.max(revision?.latestSequence ?? 0, receipt.latestSequence),
      latestTimestampMs: Math.max(
        revision?.latestTimestampMs ?? 0,
        receipt.latestTimestampMs
      )
    });
    const applyReceipt = (current: AgentState) => {
      const queuedMessages = receipt.message
        ? mergeQueuedAgentMessage(current.queuedMessages, receipt.message)
        : current.queuedMessages.filter((message) => message.id !== receipt.queueId);
      return {
        ...current,
        status: receipt.cancelledActiveRun ? "cancelled" : current.status,
        canCancel: receipt.cancelledActiveRun ? false : current.canCancel,
        canRetry: receipt.cancelledActiveRun ? true : current.canRetry,
        eventCount: Math.max(current.eventCount, receipt.eventCount),
        latestSequence: Math.max(current.latestSequence, receipt.latestSequence),
        queuedMessages
      };
    };
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    if (cached) sessionRuntimeCache.rememberAgent(sessionId, applyReceipt(cached));
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = applyReceipt(current);
        sessionRuntimeCache.rememberAgent(sessionId, next);
        return next;
      });
    }
  }

  async function drainQueuedMessages(sessionId: string) {
    if (
      queueDrainingSessionIdsRef.current.has(sessionId) ||
      suppressQueueDrainSessionIdsRef.current.has(sessionId)
    ) {
      return;
    }
    queueDrainingSessionIdsRef.current.add(sessionId);
    markSessionBusy(sessionId, true);
    try {
      while (!suppressQueueDrainSessionIdsRef.current.has(sessionId)) {
        const next = await runNextQueuedAgentMessage(sessionId);
        if (!next) {
          break;
        }
        applyAgentStateForSession(sessionId, next);
        if (activeSessionIdRef.current === sessionId) {
          setComposerError(next.lastError);
          setStreamResetVersion((version) => version + 1);
        }
        if (
          next.queuedMessages.length === 0 ||
          next.status !== "completed" ||
          next.canContinue ||
          next.pendingApprovals.length > 0 ||
          Boolean(next.lastError)
        ) {
          break;
        }
      }
      setProjectSessionState(await getProjectSessionState());
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      const message = error instanceof Error ? error.message : String(error);
      try {
        const failedState = await getAgentState(sessionId);
        applyAgentStateForSession(sessionId, failedState);
      } catch {
        // Preserve the original queue error when a follow-up state read also fails.
      }
      if (activeSessionIdRef.current === sessionId) setComposerError(message);
    } finally {
      queueDrainingSessionIdsRef.current.delete(sessionId);
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
    }
  }

  async function handleEditQueuedMessage(queueId: string, prompt: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) {
      const error = new Error("queued message not found");
      setComposerError(error.message);
      throw error;
    }
    const optimistic = { ...previous, prompt: prompt.trim(), updatedAtMs: Date.now() };
    optimisticQueuedMessagesRef.current.set(queueId, optimistic);
    updateQueuedMessagesForSession(sessionId, (messages) =>
      mergeQueuedAgentMessage(messages, optimistic)
    );
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await editQueuedAgentMessage(sessionId, queueId, prompt);
      optimisticQueuedMessagesRef.current.delete(queueId);
      applyQueuedMessageActionReceiptForSession(sessionId, receipt);
    } catch (error) {
      optimisticQueuedMessagesRef.current.delete(queueId);
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, previous)
      );
      setComposerError(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleDeleteQueuedMessage(queueId: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) return;
    optimisticallyDeletedQueuedMessagesRef.current.set(queueId, sessionId);
    updateQueuedMessagesForSession(sessionId, (messages) =>
      messages.filter((message) => message.id !== queueId)
    );
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await deleteQueuedAgentMessage(sessionId, queueId);
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      applyQueuedMessageActionReceiptForSession(sessionId, receipt);
    } catch (error) {
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, previous)
      );
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleSteerQueuedMessage(queueId: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) return;
    const runCommandActive = busySessionIds.has(sessionId);
    suppressQueueDrainSessionIdsRef.current.delete(sessionId);
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await steerQueuedAgentMessage(sessionId, queueId);
      const optimisticSteerMessage = committedSteerUserMessage(previous, receipt);
      if (optimisticSteerMessage) {
        addOptimisticUserMessage(sessionId, optimisticSteerMessage);
        optimisticallyDeletedQueuedMessagesRef.current.set(queueId, sessionId);
        steeredQueuedMessageIdsRef.current.add(queueId);
        applyQueuedMessageActionReceiptForSession(sessionId, { ...receipt, message: null });
      } else {
        applyQueuedMessageActionReceiptForSession(sessionId, receipt);
      }
      if (!runCommandActive && !receipt.steerCommitted) void drainQueuedMessages(sessionId);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleSendPrompt(value: string) {
    const nextPrompt = value.trim();
    const sessionId = activeSession?.id;
    const attachments = composerAttachments;
    if (
      (!nextPrompt && attachments.length === 0) ||
      !sessionId ||
      attachmentBusy
    ) {
      return;
    }
    const visiblePrompt =
      nextPrompt || `Review attached ${attachments.map((attachment) => attachment.name).join(", ")}`;
    const sessionAgentState =
      activeAgentState?.sessionId === sessionId
        ? activeAgentState
        : sessionRuntimeCache.peekAgent(sessionId);
    if (
      busySessionIds.has(sessionId) ||
      sessionAgentState?.status === "running" ||
      sessionAgentState?.status === "waiting_for_permission" ||
      sessionAgentState?.canCancel
    ) {
      setComposerError(null);
      const queueId = queuedMessageClientId();
      const queuedAt = Date.now();
      const optimisticMessage: QueuedAgentMessage = {
        id: queueId,
        sessionId,
        prompt: visiblePrompt,
        attachments,
        effort: agentEffort,
        mode: "queue",
        createdAtMs: queuedAt,
        updatedAtMs: queuedAt
      };
      optimisticQueuedMessagesRef.current.set(queueId, optimisticMessage);
      setPersistingQueuedMessageIds((current) => {
        const next = new Set(current);
        next.add(queueId);
        return next;
      });
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, optimisticMessage)
      );
      clearAttachments(sessionId);
      try {
        const receipt = await queueAgentMessage(
          nextPrompt,
          sessionId,
          attachments,
          agentEffort,
          queueId
        );
        optimisticQueuedMessagesRef.current.delete(queueId);
        applyQueuedMessageReceiptForSession(sessionId, receipt);
      } catch (error) {
        optimisticQueuedMessagesRef.current.delete(queueId);
        updateQueuedMessagesForSession(sessionId, (messages) =>
          messages.filter((message) => message.id !== queueId)
        );
        restoreAttachmentsIfEmpty(sessionId, attachments);
        restoreDraftIfEmpty(sessionId, value);
        setComposerError(error instanceof Error ? error.message : String(error));
      } finally {
        setPersistingQueuedMessageIds((current) => {
          if (!current.has(queueId)) return current;
          const next = new Set(current);
          next.delete(queueId);
          return next;
        });
      }
      return;
    }
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    clearAttachments(sessionId);
    markSessionBusy(sessionId, true);
    const submittedAt = Date.now();
    const optimisticUserMessage: ChatMessageView = {
      role: "user",
      content: visiblePrompt,
      timestampMs: submittedAt,
      attachments
    };
    addOptimisticUserMessage(sessionId, optimisticUserMessage);
    const runBudgetPatch = optimisticRunBudgetPatch(runtime?.agentRunBudgets, agentEffort);
    setAgentState((current) => {
      if (!current) return current;
      const contextTokensUsed =
        current.contextTokensUsed + Math.ceil(visiblePrompt.length / 4) + attachments.length * 64 + 6;
      return {
        ...current,
        sessionName: current.sessionName,
        status: "running",
        canCancel: true,
        canRetry: false,
        canContinue: false,
        ...runBudgetPatch,
        transcriptMessages: current.transcriptMessages + 1,
        contextTokensUsed,
        contextRemainingPercent: Math.max(
          0,
          ((current.contextWindowTokens - contextTokensUsed) / current.contextWindowTokens) * 100
        ),
        contextUsageEstimated: true,
        runStartedAtMs: submittedAt,
        messages: current.messages
      };
    });
    let completedState: AgentState | null = null;
    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const next = await runAgentTask(nextPrompt, sessionId, attachments, agentEffort);
      completedState = next;
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status, next.canContinue);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, next)
          )
        );
        setComposerError(next.lastError);
        setStreamResetVersion((version) => version + 1);
      }
      setProjectSessionState(await getProjectSessionState());
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      restoreAttachmentsIfEmpty(sessionId, attachments);
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const failedState = await getAgentState(sessionId);
        acknowledgeOptimisticUserMessage(sessionId, failedState.messages);
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, failedState)
          )
        );
      }
    } finally {
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
      const suppressDrain = suppressQueueDrainSessionIdsRef.current.delete(sessionId);
      if (
        !suppressDrain &&
        completedState &&
        completedState.queuedMessages.length > 0 &&
        !completedState.canContinue &&
        !completedState.lastError &&
        (completedState.status === "completed" || completedState.status === "cancelled")
      ) {
        void drainQueuedMessages(sessionId);
      }
    }
  }

  async function handleCancelAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId) return;
    suppressQueueDrainSessionIdsRef.current.add(sessionId);
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    try {
      const next = await cancelAgentTask(sessionId);
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status, next.canContinue);
      markSessionBusy(sessionId, false);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, next)
          )
        );
        setComposerError(next.lastError);
      }
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    }
  }

  async function handleRetryAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId || busySessionIds.has(sessionId)) return;
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    markSessionBusy(sessionId, true);
    let completedState: AgentState | null = null;
    try {
      const next = await retryAgentTask(sessionId);
      completedState = next;
      applyAgentStateForSession(sessionId, next);
      if (activeSessionIdRef.current === sessionId) setComposerError(next.lastError);
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      markSessionBusy(sessionId, false);
      if (
        completedState?.status === "completed" &&
        !completedState.canContinue &&
        !completedState.lastError &&
        completedState.queuedMessages.length > 0
      ) {
        void drainQueuedMessages(sessionId);
      }
    }
  }

  async function handleResolveAgentPermission(
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny",
    targetSessionId = activeSession?.id
  ) {
    const sessionId = targetSessionId;
    if (!sessionId || busySessionIds.has(sessionId)) return;
    markSessionBusy(sessionId, true);
    setComposerError(null);
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) =>
        current
          ? {
              ...current,
              status: "running",
              canCancel: true,
              canRetry: false,
              canContinue: false,
              pendingApprovals: current.pendingApprovals.filter(
                (approval) => approval.requestId !== requestId
              )
            }
          : current
      );
    }
    let completedState: AgentState | null = null;
    try {
      const next = await resolveAgentPermission(requestId, decision, sessionId);
      completedState = next;
      applyAgentStateForSession(sessionId, next);
      if (activeSessionIdRef.current === sessionId) setComposerError(next.lastError);
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const failedState = await getAgentState(sessionId);
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, failedState)
          )
        );
      }
    } finally {
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
      if (
        completedState?.status === "completed" &&
        !completedState.canContinue &&
        !completedState.lastError &&
        completedState.queuedMessages.length > 0
      ) {
        void drainQueuedMessages(sessionId);
      }
    }
  }

  async function handleExportAgentTrace() {
    setTraceBusy(true);
    setComposerError(null);
    try {
      const next = await exportAgentTraceJsonl(activeSession?.id);
      setAgentTraceState(next);
      setInspectorTab("trace");
      setInspectorOpen(true);
      const latestStep = latestTraceStep(next.turns);
      setSelectedTraceStepId((current) => current ?? latestStep?.id ?? null);
      setComposerError(next.lastError);
      if (next.exportPath) {
        await revealArtifact(next.exportPath);
      }
    } finally {
      setTraceBusy(false);
    }
  }

  useEffect(() => {
    const sessionId = activeAgentState?.sessionId;
    if (
      !sessionId ||
      activeSessionBusy ||
      activeAgentState.queuedMessages.length === 0 ||
      activeAgentState.canContinue ||
      activeAgentState.pendingApprovals.length > 0 ||
      !["idle", "completed"].includes(activeAgentState.status)
    ) {
      return;
    }
    void drainQueuedMessages(sessionId);
  }, [
    activeAgentState?.canContinue,
    activeAgentState?.pendingApprovals.length,
    activeAgentState?.queuedMessages.length,
    activeAgentState?.sessionId,
    activeAgentState?.status,
    activeSessionBusy
  ]);

  return (
    <main
      className="app-shell"
      data-active-view={activeView}
      data-sidebar-open={sidebarOpen}
      data-sidebar-resizing={sidebarResizing}
      data-inspector-open={inspectorOpen}
      data-inspector-resizing={inspectorResizing}
      style={
        {
          "--sidebar-width": `${sidebarWidth}px`,
          "--inspector-width": `${inspectorWidth}px`
        } as CSSProperties
      }
    >
      <WorkspaceChrome
        activeView={activeView}
        contextEstimated={activeAgentState?.contextUsageEstimated ?? false}
        contextRemainingPercent={activeAgentState?.contextRemainingPercent ?? 100}
        contextTokensUsed={activeAgentState?.contextTokensUsed ?? 0}
        contextWindowTokens={
          activeAgentState?.contextWindowTokens ??
          phase4?.provider.contextWindowTokens ??
          128000
        }
        inspectorOpen={inspectorOpen}
        onInspectorToggle={() => setInspectorOpen((open) => !open)}
        onSidebarToggle={() => setSidebarOpen((open) => !open)}
        sidebarOpen={sidebarOpen}
        statusText={statusText}
        title={
          activeView === "settings"
            ? "Settings"
            : activeView === "schedule"
              ? "Schedule"
              : activeSession?.name ?? "Session"
        }
      />

      <div className="settings-transition-backdrop" aria-hidden="true" />

      {activeView !== "settings" && sidebarOpen && (
        <div
          className="sidebar-resize-handle"
          role="separator"
          aria-label="Resize sidebar"
          aria-orientation="vertical"
          aria-valuemin={200}
          aria-valuemax={320}
          aria-valuenow={sidebarWidth}
          tabIndex={0}
          onPointerDown={beginSidebarResize}
          onKeyDown={handleSidebarResizeKeyDown}
        />
      )}

      {activeView !== "settings" && (
        <Sidebar
          activeView={activeView}
          projects={projects}
          sessions={sessions}
          busy={projectSessionBusy}
          searchOpen={sidebarSearchOpen}
          searchQuery={sidebarQuery}
          selectedScheduleId={selectedScheduleId}
          projectCreateOpen={projectCreateOpen}
          projectName={newProjectName}
          onViewChange={handleWorkspaceViewChange}
          onScheduleSelect={setSelectedScheduleId}
          onSearchToggle={() => {
            setSidebarSearchOpen((open) => !open);
            if (sidebarSearchOpen) setSidebarQuery("");
          }}
          onSearchQueryChange={setSidebarQuery}
          onProjectCreateToggle={() => setProjectCreateOpen((open) => !open)}
          onProjectNameChange={setNewProjectName}
          onProjectCreate={() => void handleCreateProject()}
          onSessionCreate={() => void handleCreateSession()}
          onProjectSelect={(projectId) => void handleSelectProject(projectId)}
          onProjectRename={(projectId, name) => void handleRenameProject(projectId, name)}
          onProjectDelete={(projectId) => void handleDeleteProject(projectId)}
          onSessionSelect={(sessionId) => void handleSelectSession(sessionId)}
          onSessionRename={(sessionId, name) => void handleRenameSession(sessionId, name)}
          onSessionFork={(sessionId) => void handleForkSession(sessionId)}
          onSessionArchive={(sessionId) => void handleArchiveSession(sessionId)}
          onSessionDelete={(sessionId) => void handleDeleteSession(sessionId)}
        />
      )}

      <section className="workspace" data-view={activeView} aria-label="Agent workspace">
        {activeView === "timeline" ? (
          <>
            <LiveSessionThread
              sessionId={activeSession?.id ?? null}
              loading={sessionLoadingId === activeSession?.id && !activeAgentState}
              messages={visibleAgentMessages}
              timeline={activeAgentState?.timeline ?? []}
              streamResetVersion={streamResetVersion}
              status={activeAgentState?.status ?? "idle"}
              runStartedAtMs={activeAgentState?.runStartedAtMs ?? 0}
              hasOlderHistory={activeAgentState?.hasOlderHistory ?? false}
              loadingOlderHistory={loadingOlderSessionId === activeSession?.id}
              selectedId={selectedThreadItem?.id ?? null}
              onLoadOlderHistory={loadOlderAgentHistory}
              onSelect={handleThreadSelection}
              onEditMessage={handleThreadMessageEdit}
              onArtifactInspect={(path) => {
                const sessionId = activeSession?.id;
                if (!sessionId) return;
                setInspectorOutputRequest({ sessionId, path, nonce: Date.now() });
                setInspectorOpen(true);
              }}
              onLinkOpenError={setComposerError}
              onStreamDone={handleAgentStreamDone}
            />

            <div
              className="composer-stack"
              data-has-queued={Boolean(activeAgentState?.queuedMessages.length)}
            >
              <QueuedMessages
                messages={activeAgentState?.queuedMessages ?? []}
                busyId={queuedMessageBusyId}
                persistingIds={persistingQueuedMessageIds}
                onSteer={handleSteerQueuedMessage}
                onEdit={handleEditQueuedMessage}
                onDelete={handleDeleteQueuedMessage}
              />
              <Composer
                value={composerDraft}
                working={agentWorking}
                canStop={agentCanCancel}
                canRetry={agentCanRetry}
                canContinue={agentCanContinue}
                error={composerError}
                focusRequest={composerFocusRequest}
                pendingApproval={agentApprovals[0] ?? null}
                permissionBusy={activeSessionBusy}
                attachments={composerAttachments}
                attachmentBusy={attachmentBusy}
                effort={agentEffort}
                sessionId={activeSession?.id ?? null}
                voiceConfigured={voiceConfigured}
                voiceTransport={voiceTransport}
                onChange={setActiveComposerDraft}
                onEffortChange={(effort) => void handleSessionEffortChange(effort)}
                onVoiceTranscript={appendComposerDraftForSession}
                onVoiceError={(message) => setComposerError(message)}
                onSend={(value) => void handleSendPrompt(value)}
                onPickAttachments={(files) => void handlePickAttachments(files)}
                onRemoveAttachment={handleRemoveAttachment}
                onCancel={() => void handleCancelAgentTask()}
                onRetry={() => void handleRetryAgentTask()}
                onDismissError={() => setComposerError(null)}
                onResolvePermission={(requestId, decision) =>
                  void handleResolveAgentPermission(requestId, decision)
                }
              />
            </div>
          </>
        ) : activeView === "schedule" ? (
          <Suspense fallback={<section className="schedule-view" aria-busy="true" />}>
            <ScheduleView
              projects={projectSessionState?.projects ?? []}
              sessions={projectSessionState?.sessions ?? []}
              onOpenSession={(sessionId) => void handleOpenScheduledSession(sessionId)}
              onBack={showTimelineView}
              requestedScheduleId={selectedScheduleId}
              onScheduleSelect={setSelectedScheduleId}
            />
          </Suspense>
        ) : (
          <Suspense fallback={<section className="settings-view" aria-busy="true" />}>
            <SettingsPage
            {...{
              activePermissionReviews,
              appearanceMode,
              archivedSessions,
              browserBusy,
              browserTarget,
              browserText,
              browserUrl,
              busySessionIds,
              collaborationModelCount,
              contextBusy,
              contextCheckpoint,
              debugAlwaysVisible,
              flushPersonalization,
              handleAddMcpServer,
              handleAnswerWithRag,
              handleAppearanceModeChange,
              handleCompactContext,
              handleIgnorePermissionReview,
              handleIndexRag,
              handleInstallSkillPackage,
              handleInstallSkillUrl,
              handleLoadProviderModels,
              handleMcpPolicy,
              handlePickWorkspace,
              handlePromptEvolutionToggle,
              handleRefreshMcpServer,
              handleRefreshSkills,
              handleRemoveMcpServer,
              handleResolvePermissionReview,
              handleRestorePermissionReview,
              handleRestoreSession,
              handleRunBrowserTool,
              handleRunTool,
              handleSavePersonalization,
              handleSaveProviderConfig,
              handleSaveSidecars,
              handleSaveWebSearch,
              handleSaveWorkspace,
              handleSearchRag,
              handleSkillPreference,
              ignoredPermissionReviews,
              imageEndpointValidation,
              knowledgeError,
              knowledgeGraphOpen,
              mcpBusy,
              mcpDraft,
              mcpState,
              permissionBusy,
              personalizationBusy,
              personalizationDraft,
              personalizationError,
              phase4,
              phase5,
              phase7,
              projectSessionBusy,
              projectSessionState,
              providerBusy,
              providerDraft,
              providerModelOptions,
              providerModels,
              providerModelsBusy,
              providerModelsError,
              providerModelsRefreshTurn,
              canUseConfiguredKey,
              ragBusy,
              ragQuery,
              ragSources,
              ragStats,
              runtime,
              selectedTool,
              selectedToolSpec,
              setBrowserTarget,
              setBrowserText,
              setBrowserUrl,
              setDebugAlwaysVisible,
              setKnowledgeGraphOpen,
              setMcpDraft,
              setProviderDraft,
              setRagQuery,
              setSelectedTool,
              setSettingsCategory,
              setSidecarDraft,
              setSkillUrl,
              settingsCategory,
              setToolInput,
              setWebSearchDraft,
              showSettingsSaved,
              showTimelineView,
              sidecarBusy,
              sidecarDraft,
              sidecarState,
              skillBusy,
              skillInstallError,
              skillPackageInputRef,
              skillRefreshTurn,
              skillState,
              skillUrl,
              toolBusy,
              toolInput,
              updatePersonalizationDraft,
              webSearchBusy,
              webSearchConfig,
              webSearchDraft,
              webSearchError,
              workspaceBusy,
              workspaceDraft,
              workspacePickerBusy
            }}
            />
          </Suspense>
        )}
      </section>

      <Inspector
        open={activeView === "timeline" && inspectorOpen}
        showDebug={debugAlwaysVisible}
        width={inspectorWidth}
        tab={inspectorTab}
        sessionId={activeSession?.id ?? null}
        outputRequest={inspectorOutputRequest}
        threadSelection={activeView === "timeline" ? selectedThreadItem : null}
        traceStep={selectedTraceStep}
        traceExportPath={agentTraceState?.exportPath ?? null}
        contextCheckpoint={contextCheckpoint}
        ragAnswer={phase7?.answer ?? null}
        ragSources={ragSources}
        browserObservations={browserObservations}
        toolResults={toolResults}
        sessionTraceSteps={activeSessionTraceSteps}
        roleSummaries={agentTraceState?.roleSummaries ?? []}
        workspaceRoot={runtime?.workspaceRoot ?? ""}
        agentStatus={activeAgentState?.status ?? "idle"}
        agentTurnCount={activeAgentState?.turnCount ?? 0}
        agentMaxTurns={activeAgentState?.maxTurns ?? 0}
        reviewCounts={{
          agent: agentApprovals.length,
          tool: toolApprovals.length,
          browser: browserApprovals.length
        }}
        onTabChange={setInspectorTab}
        onTraceStepSelect={(stepId) => {
          setSelectedTraceStepId(stepId);
          setSelectedThreadItem(null);
          setInspectorTab("details");
          setInspectorOpen(true);
        }}
        onTraceExport={() => void handleExportAgentTrace()}
        traceBusy={traceBusy}
        onWidthChange={setInspectorWidth}
        onResizeStart={() => setInspectorResizing(true)}
        onResizeEnd={() => setInspectorResizing(false)}
        onReview={() => {
          if (activeView !== "settings") setWorkspaceViewBeforeSettings(activeView);
          setInspectorOpenBeforeSettings(inspectorOpen);
          setActiveView("settings");
          setSettingsCategory("permissions");
          setInspectorOpen(false);
        }}
      />
      {settingsToast && (
        <div
          className="settings-saved-toast"
          key={settingsToast.id}
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          <CheckCircle2 aria-hidden="true" />
          <span>{settingsToast.message}</span>
        </div>
      )}
    </main>
  );
}
