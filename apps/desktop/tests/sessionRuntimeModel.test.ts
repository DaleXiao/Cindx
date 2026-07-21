import assert from "node:assert/strict";
import test from "node:test";
import {
  latestTraceStep,
  mergeAgentStateSnapshot,
  mergeQueuedAgentMessage,
  mergeSequencedItems,
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
