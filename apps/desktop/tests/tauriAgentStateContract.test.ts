import assert from "node:assert/strict";
import test from "node:test";
import {
  decodeNativeAgentState,
  decodeNativeAgentStateDelta
} from "../src/tauriAgentStateContract.ts";

function validState() {
  return {
    taskId: "task-a",
    projectId: "project-a",
    projectName: "Project",
    sessionId: "session-a",
    sessionName: "Session",
    status: "running",
    turnCount: 1,
    maxTurns: 10,
    transcriptMessages: 2,
    contextTokensUsed: 100,
    contextWindowTokens: 1000,
    contextRemainingPercent: 90,
    contextUsageEstimated: false,
    runStartedAtMs: 1000,
    runBudgetMs: 60000,
    runModelCallBudget: 20,
    runToolCallBudget: 48,
    canCancel: true,
    canRetry: false,
    canContinue: false,
    eventCount: 4,
    latestSequence: 4,
    oldestSequence: 1,
    hasOlderHistory: false,
    timeline: [{ sequence: 1 }],
    messages: [{ sequence: 2 }],
    pendingApprovals: [],
    queuedMessages: [],
    pendingPlanConfirmation: null,
    latestAnswer: null,
    lastError: null
  };
}

test("a contract-shaped agent state decodes", () => {
  const decoded = decodeNativeAgentState(validState());
  assert.equal(decoded.sessionId, "session-a");
  assert.equal(decoded.status, "running");
});

test("the context remaining percent accepts fractional wire values", () => {
  // The native view computes the percentage as f64 (e.g. 85.5); the decoder
  // must not require an integer here.
  const fractional = { ...validState(), contextRemainingPercent: 85.5 };
  assert.equal(decodeNativeAgentState(fractional).contextRemainingPercent, 85.5);
  const negative = { ...validState(), contextRemainingPercent: -1 };
  assert.throws(() => decodeNativeAgentState(negative), /non-negative finite number/);
});

test("agent state decoding is fail-closed against contract drift", () => {
  for (const mutate of [
    (state: any) => delete state.taskId,
    (state: any) => (state.status = "working"),
    (state: any) => (state.turnCount = "1"),
    (state: any) => (state.canCancel = "yes"),
    (state: any) => (state.timeline = { sequence: 1 }),
    (state: any) => (state.timeline = [{ id: 1 }]),
    (state: any) => (state.pendingApprovals = null),
    (state: any) => (state.latestAnswer = 42),
    (state: any) => (state.contextRemainingPercent = -1)
  ]) {
    const drifted = validState();
    mutate(drifted);
    assert.throws(() => decodeNativeAgentState(drifted), /must be/);
  }
});

test("agent state decoding rejects non-object payloads", () => {
  for (const value of [null, undefined, "state", 42, []]) {
    assert.throws(() => decodeNativeAgentState(value), /must be an object/);
  }
});

test("an agent state delta decodes only with a decodable inner state", () => {
  const valid = { reset: false, latestSequence: 4, state: validState() };
  assert.equal(decodeNativeAgentStateDelta(valid).latestSequence, 4);

  const driftedState = { ...valid, state: { ...validState(), status: "working" } };
  assert.throws(() => decodeNativeAgentStateDelta(driftedState), /must be a known agent status/);

  const driftedDelta = { ...valid, reset: "no" };
  assert.throws(() => decodeNativeAgentStateDelta(driftedDelta), /must be a boolean/);
});

test("a pending plan confirmation must be an object when present", () => {
  const drifted = { ...validState(), pendingPlanConfirmation: "plan" };
  assert.throws(() => decodeNativeAgentState(drifted), /must be an object/);
  const absent = decodeNativeAgentState(validState());
  assert.equal(absent.pendingPlanConfirmation, null);
});
