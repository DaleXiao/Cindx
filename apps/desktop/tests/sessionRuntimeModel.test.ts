import assert from "node:assert/strict";
import test from "node:test";
import {
  committedSteerReconciliation,
  committedSteerUserMessage,
  latestTraceStep,
  mergeAcknowledgedSessionActivity,
  messagesWithOptimisticUserMessages,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
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
