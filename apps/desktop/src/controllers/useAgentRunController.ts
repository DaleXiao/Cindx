import { useState, type Dispatch, type SetStateAction } from "react";
import { queuedMessageClientId } from "../appShellModel";
import { optimisticRunBudgetPatch } from "../agentRunBudgetModel";
import {
  cancelAgentTask,
  deleteQueuedAgentMessage,
  editQueuedAgentMessage,
  getAgentState,
  getProjectSessionState,
  queueAgentMessage,
  resolveAgentPermission,
  retryAgentTask,
  runAgentTask,
  steerQueuedAgentMessage
} from "../tauri";
import type {
  AgentAttachment,
  AgentEffort,
  AgentState,
  ChatMessageView,
  QueuedAgentMessage,
  QueuedAgentMessageActionReceipt,
  QueuedAgentMessageReceipt,
  RuntimeStatus
} from "../tauri";
import {
  committedSteerUserMessage,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
  mergeQueuedMessageActionReceipt,
  mergeQueuedMessageReceipt
} from "../sessionRuntimeModel";
import { planModeForSubmission } from "../planModeModel";
import type { useSessionRuntimeController } from "./useSessionRuntimeController";

type SessionRuntime = ReturnType<typeof useSessionRuntimeController>;

type AgentRunControllerInput = {
  sessionRuntime: SessionRuntime;
  activeAgentState: AgentState | null;
  activeSession: { id: string } | null;
  agentEffort: AgentEffort;
  attachmentBusy: boolean;
  clearAttachments: (sessionId: string) => void;
  composerAttachments: AgentAttachment[];
  planFirstEnabled: boolean;
  refreshPermissionReviews: () => Promise<unknown>;
  restoreAttachmentsIfEmpty: (sessionId: string, previous: AgentAttachment[]) => void;
  restoreDraftIfEmpty: (sessionId: string, value: string) => void;
  runtime: RuntimeStatus | null;
  setComposerError: Dispatch<SetStateAction<string | null>>;
};

export function useAgentRunController({
  sessionRuntime,
  activeAgentState,
  activeSession,
  agentEffort,
  attachmentBusy,
  clearAttachments,
  composerAttachments,
  planFirstEnabled,
  refreshPermissionReviews,
  restoreAttachmentsIfEmpty,
  restoreDraftIfEmpty,
  runtime,
  setComposerError
}: AgentRunControllerInput) {
  const {
    activeSessionIdRef,
    agentState,
    addOptimisticUserMessage,
    acknowledgeOptimisticUserMessage,
    applyAgentStateForSession,
    busySessionIds,
    drainQueuedMessages,
    markSessionBusy,
    optimisticQueuedMessagesRef,
    optimisticallyDeletedQueuedMessagesRef,
    preserveOptimisticQueuedMessages,
    refreshAgentTrace,
    sessionRuntimeCache,
    setAgentState,
    setProjectSessionState,
    setStreamResetVersion,
    steeredQueuedMessageIdsRef,
    suppressQueueDrainSessionIdsRef,
    updateSessionStatus
  } = sessionRuntime;
  const [queuedMessageBusyId, setQueuedMessageBusyId] = useState<string | null>(null);
  const [persistingQueuedMessageIds, setPersistingQueuedMessageIds] = useState<Set<string>>(
    () => new Set()
  );

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
    // Deliberately does NOT advance agentStateRevisionsRef: the receipt reports
    // the session's newest server revision, but only the queued-message change
    // has been applied here. Advancing the applied-event cursor would make the
    // next delta poll skip every event (tool activity, messages) produced
    // between the last poll and this enqueue (R4).
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    if (cached) {
      sessionRuntimeCache.rememberAgent(sessionId, mergeQueuedMessageReceipt(cached, receipt));
    }
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = mergeQueuedMessageReceipt(current, receipt);
        sessionRuntimeCache.rememberAgent(sessionId, next);
        return next;
      });
    }
  }

  function applyQueuedMessageActionReceiptForSession(
    sessionId: string,
    receipt: QueuedAgentMessageActionReceipt
  ) {
    // Same cursor rule as the queue receipt above: merge the visible state,
    // leave the applied-event cursor where the client actually is (R4).
    const cached = sessionRuntimeCache.peekAgent(sessionId);
    if (cached) {
      sessionRuntimeCache.rememberAgent(
        sessionId,
        mergeQueuedMessageActionReceipt(cached, receipt)
      );
    }
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = mergeQueuedMessageActionReceipt(current, receipt);
        sessionRuntimeCache.rememberAgent(sessionId, next);
        return next;
      });
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
    // Plan mode comes from the persisted Settings plan-first toggle; the
    // High/Xhigh gate normalizes it away on every other tier.
    const planMode = planModeForSubmission(agentEffort, planFirstEnabled);
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
        planMode,
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
        const receipt = await queueAgentMessage(nextPrompt, sessionId, attachments, agentEffort, queueId, planMode);
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
      const next = await runAgentTask(nextPrompt, sessionId, attachments, agentEffort, planMode);
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
    targetSessionId = activeSession?.id,
    grantCommandPrefix = false,
    subagentHint = false
  ) {
    const sessionId = targetSessionId;
    if (!sessionId) return;
    // A write subagent's patch approval arrives while its parent run is still
    // in flight: only those approvals may resolve without waiting for the run
    // command to return, and resolving one must not disturb the run's busy
    // bookkeeping. The cached agent state of a busy *background* session can
    // lag behind the review list (nothing polls it while its run command is in
    // flight), so the backend-provided review flag is an authoritative
    // fallback — a lookup miss must not silently inert the click (R1).
    const runCommandInFlight = busySessionIds.has(sessionId);
    const pendingApproval = (
      activeAgentState?.sessionId === sessionId
        ? activeAgentState
        : sessionRuntimeCache.peekAgent(sessionId)
    )?.pendingApprovals.find((approval) => approval.requestId === requestId);
    const subagentApproval = pendingApproval?.subagent ?? subagentHint;
    if (runCommandInFlight && !subagentApproval) return;
    if (!runCommandInFlight) markSessionBusy(sessionId, true);
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
      const next = await resolveAgentPermission(requestId, decision, sessionId, grantCommandPrefix);
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
      if (!runCommandInFlight) markSessionBusy(sessionId, false);
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

  return {
    handleCancelAgentTask,
    handleDeleteQueuedMessage,
    handleEditQueuedMessage,
    handleResolveAgentPermission,
    handleRetryAgentTask,
    handleSendPrompt,
    handleSteerQueuedMessage,
    persistingQueuedMessageIds,
    queuedMessageBusyId
  };
}
