import assert from "node:assert/strict";
import test from "node:test";
import {
  committedSteerReconciliation,
  committedSteerUserMessage,
  dedupeAdjacentAssistantMessages,
  latestTraceStep,
  mergeAcknowledgedSessionActivity,
  messagesWithOptimisticUserMessages,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
  mergeQueuedMessageActionReceipt,
  mergeQueuedMessageReceipt,
  mergeSequencedItems,
  projectSessionResultAsRead,
  readSessionState,
  rememberSessionState
} from "../src/sessionRuntimeModel.ts";

test("session cache remains bounded and promotes reads", () => {
  const cache = new Map<string, number>();
  rememberSessionState(cache, "a", 1, 2);
  rememberSessionState(cache, "b", 2, 2);
  assert.equal(readSessionState(cache, "a"), 1);
  rememberSessionState(cache, "c", 3, 2);

  assert.deepEqual([...cache.keys()], ["a", "c"]);
});

test("sequenced merge preserves history and replaces overlapping snapshots", () => {
  const current = [{ sequence: 1, value: "old-1" }, { sequence: 3, value: "old-3" }];
  const incoming = [{ sequence: 2, value: "new-2" }, { sequence: 3, value: "new-3" }];

  assert.deepEqual(mergeSequencedItems(current, incoming), [
    { sequence: 1, value: "old-1" },
    { sequence: 2, value: "new-2" },
    { sequence: 3, value: "new-3" }
  ]);
});

test("state snapshot merge retains an already loaded earlier history page", () => {
  const current = {
    sessionId: "session-a",
    oldestSequence: 1,
    hasOlderHistory: false,
    timeline: [{ sequence: 1 }, { sequence: 2 }],
    messages: [{ sequence: 1 }]
  } as any;
  const incoming = {
    sessionId: "session-a",
    oldestSequence: 2,
    hasOlderHistory: true,
    timeline: [{ sequence: 2 }, { sequence: 3 }],
    messages: [{ sequence: 3 }]
  } as any;
  const merged = mergeAgentStateSnapshot(current, incoming);

  assert.equal(merged.oldestSequence, 1);
  assert.equal(merged.hasOlderHistory, false);
  assert.deepEqual(merged.timeline.map((item: any) => item.sequence), [1, 2, 3]);
});

test("queue merge keeps the latest steer ahead of queued prompts", () => {
  const prompt = {
    id: "prompt",
    mode: "queue",
    createdAtMs: 1,
    updatedAtMs: 1
  } as any;
  const oldSteer = {
    id: "old-steer",
    mode: "steer",
    createdAtMs: 2,
    updatedAtMs: 2
  } as any;
  const newSteer = {
    id: "new-steer",
    mode: "steer",
    createdAtMs: 3,
    updatedAtMs: 3
  } as any;

  const merged = mergeQueuedAgentMessage([prompt, oldSteer], newSteer);
  assert.deepEqual(merged.map((item) => item.id), ["new-steer", "old-steer", "prompt"]);
});

test("latest trace step skips empty turns without flattening the trace", () => {
  const latest = latestTraceStep([
    { steps: [{ id: "first" }] },
    { steps: [] },
    { steps: [{ id: "last" }] },
    { steps: [] }
  ] as any);
  assert.equal(latest?.id, "last");
});

test("optimistic steers render immediately and reconcile by queue id", () => {
  const optimistic = {
    role: "user",
    content: "Use the existing output",
    timestampMs: 10,
    queueId: "steer-a"
  } as any;

  assert.deepEqual(
    messagesWithOptimisticUserMessages([], [optimistic]),
    [optimistic]
  );

  const persisted = { ...optimistic, sequence: 7, timestampMs: 20 };
  assert.deepEqual(
    messagesWithOptimisticUserMessages([persisted], [optimistic]),
    [persisted]
  );
});

test("steer messages project into chat only after the backend commits them", () => {
  const queued = {
    id: "steer-a",
    sessionId: "session-a",
    prompt: "Use the existing output",
    attachments: [],
    effort: "auto",
    mode: "queue",
    createdAtMs: 10,
    updatedAtMs: 10
  } as any;
  const receipt = {
    queueId: queued.id,
    message: queued,
    eventCount: 2,
    latestSequence: 2,
    latestTimestampMs: 20,
    cancelledActiveRun: false,
    steerCommitted: false
  } as any;

  assert.equal(committedSteerUserMessage(queued, receipt), null);
  assert.deepEqual(committedSteerUserMessage(queued, { ...receipt, steerCommitted: true }), {
    role: "user",
    content: queued.prompt,
    timestampMs: receipt.latestTimestampMs,
    queueId: queued.id,
    attachments: queued.attachments
  });
});

test("committed steers reconcile only after durable application or terminal restoration", () => {
  const queued = { id: "steer-a" } as any;
  const running = {
    status: "running",
    canCancel: true,
    messages: [],
    queuedMessages: [queued]
  } as any;
  assert.equal(committedSteerReconciliation(running, queued.id), "pending");
  assert.equal(
    committedSteerReconciliation(
      { ...running, status: "failed", canCancel: false },
      queued.id
    ),
    "restored"
  );
  assert.equal(
    committedSteerReconciliation(
      {
        ...running,
        messages: [{ role: "user", queueId: queued.id }],
        queuedMessages: []
      },
      queued.id
    ),
    "applied"
  );
  assert.equal(
    committedSteerReconciliation(
      { ...running, status: "completed", canCancel: false, queuedMessages: [] },
      queued.id
    ),
    "discarded"
  );
});

test("equal steer prompts reconcile independently", () => {
  const first = {
    role: "user",
    content: "Continue",
    timestampMs: 10,
    queueId: "steer-a"
  } as any;
  const second = { ...first, timestampMs: 11, queueId: "steer-b" };
  const persistedFirst = { ...first, sequence: 7, timestampMs: 20 };

  assert.deepEqual(
    messagesWithOptimisticUserMessages([persistedFirst], [first, second]),
    [second, persistedFirst]
  );
});

test("leaving a read session clears only unread terminal output", () => {
  const completed = {
    id: "session-a",
    status: "Completed",
    activity: "complete",
    attentionReason: null,
    unseenResult: true,
    latestSequence: 12,
    active: true
  } as any;
  const permission = {
    ...completed,
    status: "Approval required",
    activity: "attention",
    attentionReason: "permission",
    unseenResult: false
  } as any;

  assert.deepEqual(projectSessionResultAsRead(completed), {
    ...completed,
    status: "Ready",
    activity: "idle",
    unseenResult: false
  });
  assert.equal(projectSessionResultAsRead(permission), permission);
  assert.equal(
    projectSessionResultAsRead({ ...completed, latestSequence: 13 }, 12).unseenResult,
    true
  );
});

test("late acknowledgements update read state without restoring stale selection", () => {
  const current = {
    id: "session-a",
    status: "Completed",
    activity: "complete",
    attentionReason: null,
    unseenResult: true,
    latestSequence: 12,
    active: false
  } as any;
  const acknowledged = {
    ...current,
    status: "Ready",
    activity: "idle",
    unseenResult: false,
    active: true
  } as any;

  assert.deepEqual(mergeAcknowledgedSessionActivity(current, acknowledged), {
    ...current,
    status: "Ready",
    activity: "idle",
    unseenResult: false
  });
  assert.equal(
    mergeAcknowledgedSessionActivity(
      { ...current, latestSequence: 13 },
      acknowledged
    ).unseenResult,
    true
  );
});

test("adjacent partial and final assistant commits collapse to the longer one", () => {
  const assistant = (content: string) =>
    ({ role: "assistant", content }) as any;
  const user = { role: "user", content: "go" } as any;

  const collapsed = dedupeAdjacentAssistantMessages([
    user,
    assistant("let me delegate the research"),
    assistant("let me delegate the research."),
  ]);
  assert.equal(collapsed.length, 2);
  assert.equal(collapsed[1].content, "let me delegate the research.");

  const reversed = dedupeAdjacentAssistantMessages([
    assistant("let me delegate the research."),
    assistant("let me delegate the research"),
  ]);
  assert.equal(reversed.length, 1);
  assert.equal(reversed[0].content, "let me delegate the research.");

  const distinct = dedupeAdjacentAssistantMessages([
    assistant("first answer"),
    assistant("second, unrelated answer"),
  ]);
  assert.equal(distinct.length, 2);
});

import { reconcileOptimisticQueuedMessages } from "../src/sessionRuntimeModel.ts";

function queuedMessage(id: string, sessionId: string, mode: "queue" | "steer") {
  return {
    id,
    sessionId,
    prompt: `prompt-${id}`,
    attachments: [],
    effort: "default",
    mode,
    planMode: false,
    createdAtMs: 1,
    updatedAtMs: 1
  } as any;
}

function emptyRecords() {
  return {
    steeredQueueIds: new Set<string>(),
    optimisticallyDeleted: new Map<string, string>(),
    optimisticUserMessages: new Map<string, any[]>(),
    optimisticQueued: new Map<string, any>()
  };
}

function baseState(overrides: Record<string, unknown> = {}) {
  return {
    sessionId: "session-a",
    status: "idle",
    canCancel: false,
    messages: [],
    queuedMessages: [],
    ...overrides
  } as any;
}

test("reconciliation merges optimistic queue entries the backend has not echoed", () => {
  const records = emptyRecords();
  records.optimisticQueued.set("q1", queuedMessage("q1", "session-a", "queue"));
  const state = baseState();

  const merged = reconcileOptimisticQueuedMessages("session-a", state, records);
  assert.deepEqual(merged.queuedMessages.map((m: any) => m.id), ["q1"]);
});

test("reconciliation ignores optimistic entries belonging to other sessions", () => {
  const records = emptyRecords();
  records.optimisticQueued.set("q-other", queuedMessage("q-other", "session-b", "queue"));
  const state = baseState();

  const merged = reconcileOptimisticQueuedMessages("session-a", state, records);
  assert.equal(merged, state, "nothing to merge should return the same object");
});

test("reconciliation hides optimistically deleted entries until the backend catches up", () => {
  const records = emptyRecords();
  records.optimisticallyDeleted.set("q1", "session-a");
  const state = baseState({ queuedMessages: [queuedMessage("q1", "session-a", "queue")] });

  const merged = reconcileOptimisticQueuedMessages("session-a", state, records);
  assert.equal(merged.queuedMessages.length, 0);
});

test("reconciliation drops a committed steer bubble but keeps a pending one", () => {
  const committed = emptyRecords();
  committed.steeredQueueIds.add("q1");
  committed.optimisticallyDeleted.set("q1", "session-a");
  committed.optimisticUserMessages.set("session-a", [
    { role: "user", content: "steered", queueId: "q1" } as any
  ]);
  // The steer was applied: a user message carries the queueId.
  const applied = baseState({
    messages: [{ role: "user", content: "steered", queueId: "q1" }]
  });
  const mergedApplied = reconcileOptimisticQueuedMessages("session-a", applied, committed);
  assert.equal(committed.steeredQueueIds.has("q1"), false);
  assert.equal(committed.optimisticUserMessages.has("session-a"), false);
  assert.equal(mergedApplied, applied);

  const pending = emptyRecords();
  pending.steeredQueueIds.add("q2");
  pending.optimisticallyDeleted.set("q2", "session-a");
  pending.optimisticUserMessages.set("session-a", [
    { role: "user", content: "steering", queueId: "q2" } as any
  ]);
  // A run is in flight, so the steer is still pending and the bubble stays.
  const inFlight = baseState({ status: "running", canCancel: true });
  reconcileOptimisticQueuedMessages("session-a", inFlight, pending);
  assert.equal(pending.steeredQueueIds.has("q2"), true);
  assert.equal(pending.optimisticUserMessages.get("session-a")?.length, 1);
});

test("queue receipt merges state without claiming unapplied events", () => {
  const current = {
    sessionId: "session-a",
    eventCount: 10,
    latestSequence: 10,
    status: "running",
    canCancel: true,
    canRetry: false,
    queuedMessages: []
  } as any;
  const receipt = {
    message: { id: "queue-1", mode: "queue", createdAtMs: 5, updatedAtMs: 5 },
    eventCount: 12,
    latestSequence: 12,
    latestTimestampMs: 60
  } as any;

  const merged = mergeQueuedMessageReceipt(current, receipt);

  // Visible counters and the queued list advance...
  assert.equal(merged.eventCount, 12);
  assert.equal(merged.latestSequence, 12);
  assert.deepEqual(merged.queuedMessages.map((message: any) => message.id), ["queue-1"]);
  // ...and nothing else about the run state is rewritten.
  assert.equal(merged.status, "running");
  assert.equal(merged.canCancel, true);
});

test("queue action receipt removes deleted messages and records cancellation", () => {
  const current = {
    sessionId: "session-a",
    eventCount: 10,
    latestSequence: 10,
    status: "running",
    canCancel: true,
    canRetry: false,
    queuedMessages: [{ id: "queue-1", mode: "queue", createdAtMs: 5, updatedAtMs: 5 }]
  } as any;

  const deleted = mergeQueuedMessageActionReceipt(current, {
    queueId: "queue-1",
    message: null,
    eventCount: 11,
    latestSequence: 11,
    latestTimestampMs: 70,
    cancelledActiveRun: false,
    steerCommitted: false
  } as any);
  assert.deepEqual(deleted.queuedMessages, []);
  assert.equal(deleted.status, "running");

  const cancelled = mergeQueuedMessageActionReceipt(current, {
    queueId: "queue-2",
    message: null,
    eventCount: 11,
    latestSequence: 11,
    latestTimestampMs: 70,
    cancelledActiveRun: true,
    steerCommitted: false
  } as any);
  assert.equal(cancelled.status, "cancelled");
  assert.equal(cancelled.canCancel, false);
  assert.equal(cancelled.canRetry, true);
});
