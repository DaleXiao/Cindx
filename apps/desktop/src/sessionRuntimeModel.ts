import type {
  AgentState,
  AgentStateDelta,
  AgentTraceRoleSummary,
  AgentTraceState,
  AgentTraceStepView,
  ChatMessageView,
  ContextState,
  ProjectSessionState,
  QueuedAgentMessage,
  QueuedAgentMessageActionReceipt,
  SessionView
} from "./tauri";

export const SESSION_STATE_CACHE_LIMIT = 24;
export const SESSION_AUXILIARY_CACHE_LIMIT = 8;

export type CachedSessionRuntimeState = {
  agent: AgentState | null;
  trace: AgentTraceState | null;
  context: ContextState | null;
};

export class SessionRuntimeCache {
  private readonly agentStates = new Map<string, AgentState>();
  private readonly traceStates = new Map<string, AgentTraceState>();
  private readonly contextStates = new Map<string, ContextState>();
  private readonly agentRequests = new Map<string, Promise<AgentState>>();

  hasAgent(sessionId: string) {
    return this.agentStates.has(sessionId);
  }

  peekAgent(sessionId: string) {
    return this.agentStates.get(sessionId) ?? null;
  }

  read(sessionId: string): CachedSessionRuntimeState {
    return {
      agent: readSessionState(this.agentStates, sessionId),
      trace: readSessionState(this.traceStates, sessionId),
      context: readSessionState(this.contextStates, sessionId)
    };
  }

  rememberAgent(sessionId: string, state: AgentState) {
    rememberSessionState(this.agentStates, sessionId, state);
  }

  rememberTrace(sessionId: string, state: AgentTraceState) {
    rememberSessionState(
      this.traceStates,
      sessionId,
      state,
      SESSION_AUXILIARY_CACHE_LIMIT
    );
  }

  rememberContext(sessionId: string, state: ContextState) {
    rememberSessionState(
      this.contextStates,
      sessionId,
      state,
      SESSION_AUXILIARY_CACHE_LIMIT
    );
  }

  requestAgent(sessionId: string, load: () => Promise<AgentState>) {
    const existing = this.agentRequests.get(sessionId);
    if (existing) return existing;
    const request = load().then((state) => {
      this.rememberAgent(sessionId, state);
      return state;
    });
    this.agentRequests.set(sessionId, request);
    void request
      .finally(() => {
        if (this.agentRequests.get(sessionId) === request) {
          this.agentRequests.delete(sessionId);
        }
      })
      .catch(() => {});
    return request;
  }

  forget(sessionId: string) {
    this.agentStates.delete(sessionId);
    this.traceStates.delete(sessionId);
    this.contextStates.delete(sessionId);
    this.agentRequests.delete(sessionId);
  }
}

export function rememberSessionState<Value>(
  cache: Map<string, Value>,
  sessionId: string,
  value: Value,
  limit = SESSION_STATE_CACHE_LIMIT
) {
  cache.delete(sessionId);
  cache.set(sessionId, value);
  while (cache.size > limit) {
    const oldestSessionId = cache.keys().next().value;
    if (!oldestSessionId) break;
    cache.delete(oldestSessionId);
  }
}

export function readSessionState<Value>(cache: Map<string, Value>, sessionId: string) {
  const value = cache.get(sessionId);
  if (value === undefined) return null;
  cache.delete(sessionId);
  cache.set(sessionId, value);
  return value;
}

export function projectSessionResultAsRead(
  session: SessionView,
  throughSequence = session.latestSequence
): SessionView {
  const isUnreadResult =
    session.unseenResult &&
    session.latestSequence <= throughSequence &&
    (session.activity === "complete" ||
      (session.activity === "attention" && session.attentionReason === "failed"));
  if (!isUnreadResult) return session;
  return {
    ...session,
    status: "Ready",
    activity: "idle",
    attentionReason: null,
    unseenResult: false
  };
}

export function projectReadSessionResult(
  state: ProjectSessionState,
  sessionId: string | null,
  throughSequence?: number
): ProjectSessionState {
  if (!sessionId) return state;
  let changed = false;
  const sessions = state.sessions.map((session) => {
    if (session.id !== sessionId) return session;
    const next = projectSessionResultAsRead(session, throughSequence);
    changed ||= next !== session;
    return next;
  });
  return changed ? { ...state, sessions } : state;
}

export function mergeAcknowledgedSessionActivity(
  current: SessionView,
  acknowledged: SessionView
): SessionView {
  if (
    current.id !== acknowledged.id ||
    acknowledged.latestSequence < current.latestSequence ||
    (current.activity === "working" && acknowledged.activity !== "working")
  ) {
    return current;
  }
  return {
    ...current,
    status: acknowledged.status,
    activity: acknowledged.activity,
    attentionReason: acknowledged.attentionReason,
    unseenResult: acknowledged.unseenResult,
    latestSequence: acknowledged.latestSequence
  };
}

export function containsOptimisticUserMessage(
  messages: ChatMessageView[],
  optimistic: ChatMessageView
) {
  if (optimistic.queueId) {
    return messages.some(
      (message) => message.role === "user" && message.queueId === optimistic.queueId
    );
  }
  return messages.some(
    (message) =>
      message.role === "user" &&
      message.content === optimistic.content &&
      message.timestampMs >= optimistic.timestampMs - 1_000
  );
}

export function messagesWithOptimisticUserMessages(
  messages: ChatMessageView[],
  optimistic: ChatMessageView[] | undefined
) {
  if (!optimistic || optimistic.length === 0) return messages;
  const pending = optimistic.filter(
    (message) => !containsOptimisticUserMessage(messages, message)
  );
  if (pending.length === 0) return messages;
  return [...messages, ...pending].sort(
    (left, right) => left.timestampMs - right.timestampMs
  );
}

export function committedSteerUserMessage(
  queued: QueuedAgentMessage,
  receipt: QueuedAgentMessageActionReceipt
): ChatMessageView | null {
  if (!receipt.steerCommitted) return null;
  return {
    role: "user",
    content: queued.prompt,
    timestampMs: receipt.latestTimestampMs,
    queueId: queued.id,
    attachments: queued.attachments
  };
}

export function committedSteerReconciliation(
  state: Pick<AgentState, "status" | "canCancel" | "messages" | "queuedMessages">,
  queueId: string
): "applied" | "restored" | "discarded" | "pending" {
  if (
    state.messages.some(
      (message) => message.role === "user" && message.queueId === queueId
    )
  ) {
    return "applied";
  }
  const runInFlight =
    state.canCancel || ["running", "waiting_for_permission"].includes(state.status);
  if (!runInFlight && state.queuedMessages.some((message) => message.id === queueId)) {
    return "restored";
  }
  return runInFlight ? "pending" : "discarded";
}

export function latestTraceStep(turns: { steps: AgentTraceStepView[] }[]) {
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const steps = turns[index].steps;
    if (steps.length > 0) return steps[steps.length - 1];
  }
  return null;
}

export function agentStateUnchanged(current: AgentState | null, next: AgentState) {
  if (!current) return false;
  const currentMessage = current.messages[current.messages.length - 1];
  const nextMessage = next.messages[next.messages.length - 1];
  const currentTimeline = current.timeline[current.timeline.length - 1];
  const nextTimeline = next.timeline[next.timeline.length - 1];
  const currentApproval = current.pendingApprovals[current.pendingApprovals.length - 1];
  const nextApproval = next.pendingApprovals[next.pendingApprovals.length - 1];

  return (
    current.taskId === next.taskId &&
    current.projectId === next.projectId &&
    current.projectName === next.projectName &&
    current.sessionId === next.sessionId &&
    current.sessionName === next.sessionName &&
    current.status === next.status &&
    current.turnCount === next.turnCount &&
    current.maxTurns === next.maxTurns &&
    current.transcriptMessages === next.transcriptMessages &&
    current.contextTokensUsed === next.contextTokensUsed &&
    current.contextWindowTokens === next.contextWindowTokens &&
    current.contextRemainingPercent === next.contextRemainingPercent &&
    current.contextUsageEstimated === next.contextUsageEstimated &&
    current.runStartedAtMs === next.runStartedAtMs &&
    current.runBudgetMs === next.runBudgetMs &&
    current.runModelCallBudget === next.runModelCallBudget &&
    current.runToolCallBudget === next.runToolCallBudget &&
    current.canCancel === next.canCancel &&
    current.canRetry === next.canRetry &&
    current.canContinue === next.canContinue &&
    current.eventCount === next.eventCount &&
    current.latestSequence === next.latestSequence &&
    current.oldestSequence === next.oldestSequence &&
    current.hasOlderHistory === next.hasOlderHistory &&
    current.latestAnswer === next.latestAnswer &&
    current.lastError === next.lastError &&
    current.messages.length === next.messages.length &&
    current.timeline.length === next.timeline.length &&
    current.pendingApprovals.length === next.pendingApprovals.length &&
    currentMessage?.role === nextMessage?.role &&
    currentMessage?.content === nextMessage?.content &&
    currentMessage?.timestampMs === nextMessage?.timestampMs &&
    currentTimeline?.label === nextTimeline?.label &&
    currentTimeline?.detail === nextTimeline?.detail &&
    currentTimeline?.state === nextTimeline?.state &&
    currentTimeline?.timestampMs === nextTimeline?.timestampMs &&
    currentApproval?.requestId === nextApproval?.requestId &&
    currentApproval?.input === nextApproval?.input
  );
}

export function mergeAgentStateDelta(
  current: AgentState | null,
  delta: AgentStateDelta
) {
  if (!current || current.sessionId !== delta.state.sessionId) return delta.state;
  return mergeAgentStateSnapshot(current, delta.state);
}

export function dedupeAdjacentAssistantMessages(messages: ChatMessageView[]) {
  const out: ChatMessageView[] = [];
  for (const message of messages) {
    const previous = out[out.length - 1];
    if (previous && message.role === "assistant" && previous.role === "assistant") {
      const current = message.content.trim();
      const prior = previous.content.trim();
      // Collapse a partial+final commit of the same answer (one is a prefix of the
      // other), keeping the longer; skip exact duplicates.
      if (current === prior) continue;
      if (current.startsWith(prior)) {
        out[out.length - 1] = message;
        continue;
      }
      if (prior.startsWith(current)) continue;
    }
    out.push(message);
  }
  return out;
}

export function mergeAgentStateSnapshot(current: AgentState | null, incoming: AgentState) {
  if (!current || current.sessionId !== incoming.sessionId) {
    return { ...incoming, messages: dedupeAdjacentAssistantMessages(incoming.messages) };
  }
  const currentHasEarlierHistory =
    current.oldestSequence > 0 &&
    (incoming.oldestSequence === 0 || current.oldestSequence <= incoming.oldestSequence);
  return {
    ...incoming,
    oldestSequence: currentHasEarlierHistory ? current.oldestSequence : incoming.oldestSequence,
    hasOlderHistory: currentHasEarlierHistory
      ? current.hasOlderHistory
      : incoming.hasOlderHistory,
    timeline: mergeSequencedItems(current.timeline, incoming.timeline),
    messages: dedupeAdjacentAssistantMessages(
      mergeSequencedItems(current.messages, incoming.messages)
    )
  };
}

export function mergeQueuedAgentMessage(
  current: QueuedAgentMessage[],
  incoming: QueuedAgentMessage
) {
  const next = current.filter((message) => message.id !== incoming.id);
  next.push(incoming);
  next.sort((left, right) => {
    if (left.mode !== right.mode) return left.mode === "steer" ? -1 : 1;
    if (left.mode === "steer") {
      return right.updatedAtMs - left.updatedAtMs || left.id.localeCompare(right.id);
    }
    return left.createdAtMs - right.createdAtMs || left.id.localeCompare(right.id);
  });
  return next;
}

export function mergeSequencedItems<Item extends { sequence?: number }>(
  current: Item[],
  incoming: Item[]
) {
  if (incoming.length === 0) return current;
  if (current.length === 0) return incoming;
  const currentFirst = current[0]?.sequence;
  const currentLast = current[current.length - 1]?.sequence;
  const incomingFirst = incoming[0]?.sequence;
  const incomingLast = incoming[incoming.length - 1]?.sequence;
  if (
    currentLast !== undefined &&
    incomingFirst !== undefined &&
    currentLast < incomingFirst
  ) {
    return [...current, ...incoming];
  }
  if (
    incomingLast !== undefined &&
    currentFirst !== undefined &&
    incomingLast < currentFirst
  ) {
    return [...incoming, ...current];
  }
  if (
    current.every((item) => item.sequence !== undefined) &&
    incoming.every((item) => item.sequence !== undefined)
  ) {
    const merged: Item[] = [];
    let currentIndex = 0;
    let incomingIndex = 0;
    while (currentIndex < current.length || incomingIndex < incoming.length) {
      const currentItem = current[currentIndex];
      const incomingItem = incoming[incomingIndex];
      if (!incomingItem) {
        merged.push(currentItem);
        currentIndex += 1;
      } else if (!currentItem) {
        merged.push(incomingItem);
        incomingIndex += 1;
      } else if (currentItem.sequence! < incomingItem.sequence!) {
        merged.push(currentItem);
        currentIndex += 1;
      } else if (incomingItem.sequence! < currentItem.sequence!) {
        merged.push(incomingItem);
        incomingIndex += 1;
      } else {
        merged.push(incomingItem);
        currentIndex += 1;
        incomingIndex += 1;
      }
    }
    return merged;
  }
  const merged = new Map<number | string, Item>();
  current.forEach((item, index) => merged.set(item.sequence ?? `current-${index}`, item));
  incoming.forEach((item, index) => merged.set(item.sequence ?? `incoming-${index}`, item));
  return [...merged.values()].sort(
    (left, right) =>
      (left.sequence ?? Number.MAX_SAFE_INTEGER) -
      (right.sequence ?? Number.MAX_SAFE_INTEGER)
  );
}

function roleSummaryUnchanged(
  current: AgentTraceRoleSummary,
  next: AgentTraceRoleSummary
) {
  return (
    current.role === next.role &&
    current.calls === next.calls &&
    current.completed === next.completed &&
    current.interrupted === next.interrupted &&
    current.degraded === next.degraded &&
    current.latencyMs === next.latencyMs &&
    current.firstTokenLatencyMs === next.firstTokenLatencyMs &&
    current.totalTokens === next.totalTokens &&
    current.evidenceCount === next.evidenceCount &&
    current.models.length === next.models.length &&
    current.models.every((model, index) => model === next.models[index])
  );
}

export function agentTraceUnchanged(
  current: AgentTraceState | null,
  next: AgentTraceState
) {
  if (!current) return false;
  const currentStep = latestTraceStep(current.turns);
  const nextStep = latestTraceStep(next.turns);
  return (
    current.taskId === next.taskId &&
    current.traceId === next.traceId &&
    current.runId === next.runId &&
    current.status === next.status &&
    current.turnCount === next.turnCount &&
    current.stepCount === next.stepCount &&
    current.finishedAtMs === next.finishedAtMs &&
    current.lastError === next.lastError &&
    current.roleSummaries.length === next.roleSummaries.length &&
    current.roleSummaries.every((summary, index) =>
      roleSummaryUnchanged(summary, next.roleSummaries[index])
    ) &&
    currentStep?.id === nextStep?.id &&
    currentStep?.status === nextStep?.status &&
    currentStep?.finishedAtMs === nextStep?.finishedAtMs &&
    currentStep?.detail === nextStep?.detail
  );
}
