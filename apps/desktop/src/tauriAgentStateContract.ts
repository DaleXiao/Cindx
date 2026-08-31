import type { NativeAgentState, NativeAgentStateDelta } from "./tauriNativeTypes.ts";

/**
 * Runtime shape validation for the agent-state DTOs returned by Tauri
 * commands. A typed Tauri invoke only asserts types at compile time; these
 * decoders fail closed at the IPC boundary when the native contract drifts,
 * matching the existing `decodeNativeRuntimeStatus` pattern. Validation is
 * structural (field presence, primitive kinds, union membership, array
 * shapes) — the committed DTO contract tests on the Rust side pin the exact
 * wire shape.
 */

type UnknownRecord = Record<string, unknown>;

const AGENT_STATUS_VALUES = [
  "idle",
  "running",
  "waiting_for_permission",
  "paused",
  "completed",
  "failed",
  "cancelled"
] as const;

function record(value: unknown, path: string): UnknownRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be an object`);
  }
  return value as UnknownRecord;
}

function string(value: unknown, path: string): string {
  if (typeof value !== "string") throw new Error(`${path} must be a string`);
  return value;
}

function nullableString(value: unknown, path: string): string | null {
  if (value !== null && typeof value !== "string") {
    throw new Error(`${path} must be a string or null`);
  }
  return value as string | null;
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${path} must be a boolean`);
  return value;
}

function nonNegativeInteger(value: unknown, path: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new Error(`${path} must be a non-negative safe integer`);
  }
  return value as number;
}

/** Percentages are f64 on the wire (e.g. 85.5); validate kind and bounds only. */
function nonNegativeNumber(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new Error(`${path} must be a non-negative finite number`);
  }
  return value;
}

function status(value: unknown, path: string): (typeof AGENT_STATUS_VALUES)[number] {
  if (!AGENT_STATUS_VALUES.includes(value as (typeof AGENT_STATUS_VALUES)[number])) {
    throw new Error(`${path} must be a known agent status`);
  }
  return value as (typeof AGENT_STATUS_VALUES)[number];
}

function objectArray(value: unknown, path: string): UnknownRecord[] {
  if (!Array.isArray(value)) throw new Error(`${path} must be an array`);
  return value.map((entry, index) => record(entry, `${path}[${index}]`));
}

function sequencedEntryArray(value: unknown, path: string): UnknownRecord[] {
  return objectArray(value, path).map((entry, index) => {
    nonNegativeInteger(entry.sequence, `${path}[${index}].sequence`);
    return entry;
  });
}

export function decodeNativeAgentState(value: unknown): NativeAgentState {
  const source = record(value, "AgentState");
  string(source.taskId, "AgentState.taskId");
  nullableString(source.projectId, "AgentState.projectId");
  nullableString(source.projectName, "AgentState.projectName");
  nullableString(source.sessionId, "AgentState.sessionId");
  nullableString(source.sessionName, "AgentState.sessionName");
  status(source.status, "AgentState.status");
  nonNegativeInteger(source.turnCount, "AgentState.turnCount");
  nonNegativeInteger(source.maxTurns, "AgentState.maxTurns");
  nonNegativeInteger(source.transcriptMessages, "AgentState.transcriptMessages");
  nonNegativeInteger(source.contextTokensUsed, "AgentState.contextTokensUsed");
  nonNegativeInteger(source.contextWindowTokens, "AgentState.contextWindowTokens");
  nonNegativeNumber(source.contextRemainingPercent, "AgentState.contextRemainingPercent");
  boolean(source.contextUsageEstimated, "AgentState.contextUsageEstimated");
  nonNegativeInteger(source.runStartedAtMs, "AgentState.runStartedAtMs");
  nonNegativeInteger(source.runBudgetMs, "AgentState.runBudgetMs");
  nonNegativeInteger(source.runModelCallBudget, "AgentState.runModelCallBudget");
  nonNegativeInteger(source.runToolCallBudget, "AgentState.runToolCallBudget");
  boolean(source.canCancel, "AgentState.canCancel");
  boolean(source.canRetry, "AgentState.canRetry");
  boolean(source.canContinue, "AgentState.canContinue");
  nonNegativeInteger(source.eventCount, "AgentState.eventCount");
  nonNegativeInteger(source.latestSequence, "AgentState.latestSequence");
  nonNegativeInteger(source.oldestSequence, "AgentState.oldestSequence");
  boolean(source.hasOlderHistory, "AgentState.hasOlderHistory");
  sequencedEntryArray(source.timeline, "AgentState.timeline");
  sequencedEntryArray(source.messages, "AgentState.messages");
  objectArray(source.pendingApprovals, "AgentState.pendingApprovals");
  objectArray(source.queuedMessages, "AgentState.queuedMessages");
  if (source.pendingPlanConfirmation !== undefined && source.pendingPlanConfirmation !== null) {
    record(source.pendingPlanConfirmation, "AgentState.pendingPlanConfirmation");
  }
  nullableString(source.latestAnswer, "AgentState.latestAnswer");
  nullableString(source.lastError, "AgentState.lastError");
  return source as unknown as NativeAgentState;
}

export function decodeNativeAgentStateDelta(value: unknown): NativeAgentStateDelta {
  const source = record(value, "AgentStateDelta");
  boolean(source.reset, "AgentStateDelta.reset");
  nonNegativeInteger(source.latestSequence, "AgentStateDelta.latestSequence");
  decodeNativeAgentState(source.state);
  return source as unknown as NativeAgentStateDelta;
}
