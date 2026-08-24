import { startTransition, useEffect, type Dispatch, type SetStateAction } from "react";
import {
  BACKGROUND_AGENT_POLL_INTERVAL_MS,
  FOREGROUND_AGENT_POLL_INTERVAL_MS,
  type WorkspaceView
} from "../appShellModel";
import {
  getAgentStateDelta,
  getAgentStateRevision,
  getAgentTraceState,
  getContextState,
  getProjectSessionState,
  subscribeToSessionTitleUpdates
} from "../tauri";
import type { AgentState, ContextState, ProjectSessionState } from "../tauri";
import {
  agentStateUnchanged,
  agentTraceUnchanged,
  latestTraceStep,
  mergeAgentStateDelta
} from "../sessionRuntimeModel";
import type { useSessionRuntimeController } from "./useSessionRuntimeController";

type SessionRuntime = ReturnType<typeof useSessionRuntimeController>;

type SessionRuntimeSyncInput = {
  sessionRuntime: SessionRuntime;
  activeAgentState: AgentState | null;
  activeSession: ProjectSessionState["sessions"][number] | null;
  activeSessionBusy: boolean;
  activeView: WorkspaceView;
  inspectorOpen: boolean;
  sessionPrefetchKey: string;
  setComposerError: Dispatch<SetStateAction<string | null>>;
  setContextState: Dispatch<SetStateAction<ContextState | null>>;
};

export function useSessionRuntimeSync({
  sessionRuntime,
  activeAgentState,
  activeSession,
  activeSessionBusy,
  activeView,
  inspectorOpen,
  sessionPrefetchKey,
  setComposerError,
  setContextState
}: SessionRuntimeSyncInput) {
  const {
    activeSessionIdRef,
    agentState,
    agentStateRevisionsRef,
    agentTraceState,
    acknowledgeOptimisticUserMessage,
    drainQueuedMessages,
    preserveOptimisticQueuedMessages,
    requestSessionAgentState,
    sessionRuntimeCache,
    setAgentState,
    setAgentTraceState,
    setProjectSessionState,
    setSelectedTraceStepId,
    updateSessionStatus
  } = sessionRuntime;

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
    activeSessionIdRef.current = activeSession?.id ?? null;
  }, [activeSession?.id]);

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
}
