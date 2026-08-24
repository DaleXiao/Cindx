import {
  useCallback,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction
} from "react";
import type { InspectorTab } from "../components/Inspector";
import type { SessionThreadSelection } from "../components/SessionThread";
import {
  acknowledgeSessionActivity,
  exportAgentTraceJsonl,
  getAgentHistoryPage,
  getAgentState,
  getAgentTraceState,
  getProjectSessionState,
  revealArtifact,
  runNextQueuedAgentMessage,
  setSessionEffort,
  setSessionModel
} from "../tauri";
import type {
  AgentEffort,
  AgentState,
  AgentTraceState,
  ChatMessageView,
  ContextState,
  ProjectSessionState,
  QueuedAgentMessage
} from "../tauri";
import { normalizedSessionEffort } from "../appShellModel";
import {
  SessionRuntimeCache,
  agentStateUnchanged,
  committedSteerReconciliation,
  containsOptimisticUserMessage,
  latestTraceStep,
  mergeAcknowledgedSessionActivity,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
  mergeSequencedItems
} from "../sessionRuntimeModel";

type SessionRuntimeControllerInput = {
  setComposerError: Dispatch<SetStateAction<string | null>>;
  setContextState: Dispatch<SetStateAction<ContextState | null>>;
  refreshPermissionReviews: () => Promise<unknown>;
  setInspectorTab: (tab: InspectorTab) => void;
  setInspectorOpen: (open: boolean) => void;
};

export function useSessionRuntimeController({
  setComposerError,
  setContextState,
  refreshPermissionReviews,
  setInspectorTab,
  setInspectorOpen
}: SessionRuntimeControllerInput) {
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [sessionLoadingId, setSessionLoadingId] = useState<string | null>(null);
  const [agentTraceState, setAgentTraceState] = useState<AgentTraceState | null>(null);
  const [selectedThreadItem, setSelectedThreadItem] =
    useState<SessionThreadSelection | null>(null);
  const [selectedTraceStepId, setSelectedTraceStepId] = useState<string | null>(null);
  const [projectSessionState, setProjectSessionState] = useState<ProjectSessionState | null>(null);
  const [streamResetVersion, setStreamResetVersion] = useState(0);
  const [traceBusy, setTraceBusy] = useState(false);
  const [busySessionIds, setBusySessionIds] = useState<Set<string>>(() => new Set());
  const activeSessionIdRef = useRef<string | null>(null);
  const agentStateRevisionsRef = useRef<
    Map<string, { eventCount: number; latestSequence: number; latestTimestampMs: number }>
  >(new Map());
  const [sessionRuntimeCache] = useState(() => new SessionRuntimeCache());
  const agentHistoryRequestsRef = useRef<Set<string>>(new Set());
  const [loadingOlderSessionId, setLoadingOlderSessionId] = useState<string | null>(null);
  const sessionSelectionRequestRef = useRef(0);
  const sessionLifecycleRefreshRef = useRef(0);
  const optimisticUserMessagesRef = useRef<Map<string, ChatMessageView[]>>(new Map());
  const [optimisticUserMessageRevision, setOptimisticUserMessageRevision] = useState(0);
  const optimisticQueuedMessagesRef = useRef<Map<string, QueuedAgentMessage>>(new Map());
  const optimisticallyDeletedQueuedMessagesRef = useRef<Map<string, string>>(new Map());
  const steeredQueuedMessageIdsRef = useRef<Set<string>>(new Set());
  const queueDrainingSessionIdsRef = useRef<Set<string>>(new Set());
  const suppressQueueDrainSessionIdsRef = useRef<Set<string>>(new Set());

  const activeSession =
    projectSessionState?.sessions.find((session) => session.active) ?? null;
  const activeAgentState =
    activeSession && agentState?.sessionId === activeSession.id ? agentState : null;
  const agentEffort = normalizedSessionEffort(activeSession?.effort);

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

  async function handleSessionModelChange(agentModel: string) {
    const sessionId = activeSession?.id;
    if (!sessionId) return;
    const previousModel = activeSession?.agentModel ?? "";
    setProjectSessionState((current) =>
      current
        ? {
            ...current,
            sessions: current.sessions.map((session) =>
              session.id === sessionId ? { ...session, agentModel } : session
            )
          }
        : current
    );
    try {
      const next = await setSessionModel(sessionId, agentModel);
      const persisted =
        next.sessions.find((session) => session.id === sessionId)?.agentModel ?? agentModel;
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === sessionId ? { ...session, agentModel: persisted } : session
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
                session.id === sessionId ? { ...session, agentModel: previousModel } : session
              )
            }
          : current
      );
      setComposerError(error instanceof Error ? error.message : String(error));
    }
  }

  function applyBootstrapProjectSessionState(state: ProjectSessionState) {
    setProjectSessionState(state);
    setSessionLoadingId(
      state.activeSessionId && !sessionRuntimeCache.hasAgent(state.activeSessionId)
        ? state.activeSessionId
        : null
    );
    setComposerError((current) => current ?? state.lastError);
  }

  function applyBootstrapAgentState(state: AgentState) {
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
  }

  return {
    activeSessionIdRef,
    agentState,
    agentStateRevisionsRef,
    agentTraceState,
    applyAgentStateForSession,
    applyBootstrapAgentState,
    applyBootstrapProjectSessionState,
    applySelectedSessionAgentState,
    acknowledgeOptimisticUserMessage,
    acknowledgeSessionResult,
    activeAgentState,
    activeSession,
    addOptimisticUserMessage,
    busySessionIds,
    drainQueuedMessages,
    handleAgentStreamDone,
    handleExportAgentTrace,
    handleSessionEffortChange,
    handleSessionModelChange,
    handleThreadSelection,
    loadOlderAgentHistory,
    loadingOlderSessionId,
    markSessionBusy,
    optimisticQueuedMessagesRef,
    optimisticUserMessageRevision,
    optimisticUserMessagesRef,
    optimisticallyDeletedQueuedMessagesRef,
    preserveOptimisticQueuedMessages,
    projectSessionState,
    refreshAgentTrace,
    requestSessionAgentState,
    restoreCachedSessionState,
    selectedThreadItem,
    selectedTraceStepId,
    sessionLifecycleRefreshRef,
    sessionLoadingId,
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
    streamResetVersion,
    steeredQueuedMessageIdsRef,
    suppressQueueDrainSessionIdsRef,
    traceBusy,
    updateSessionStatus
  };
}
