import type {
  NativeAgentRunBudgetView,
  NativeAgentRunBudgetsView,
  NativeRuntimeStatus
} from "./tauriNativeTypes.ts";

type UnknownRecord = Record<string, unknown>;

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

function stringArray(value: unknown, path: string): string[] {
  if (!Array.isArray(value)) throw new Error(`${path} must be an array`);
  value.forEach((entry, index) => string(entry, `${path}[${index}]`));
  return value as string[];
}

function runBudget(value: unknown, path: string): NativeAgentRunBudgetView {
  const source = record(value, path);
  nonNegativeInteger(source.maxDurationMs, `${path}.maxDurationMs`);
  nonNegativeInteger(source.maxModelCalls, `${path}.maxModelCalls`);
  nonNegativeInteger(source.maxToolCalls, `${path}.maxToolCalls`);
  return source as NativeAgentRunBudgetView;
}

function runBudgets(value: unknown): NativeAgentRunBudgetsView {
  const source = record(value, "RuntimeStatus.agentRunBudgets");
  runBudget(source.fast, "RuntimeStatus.agentRunBudgets.fast");
  runBudget(source.auto, "RuntimeStatus.agentRunBudgets.auto");
  runBudget(source.pro, "RuntimeStatus.agentRunBudgets.pro");
  return source as NativeAgentRunBudgetsView;
}

export function decodeNativeRuntimeStatus(value: unknown): NativeRuntimeStatus {
  const source = record(value, "RuntimeStatus");
  string(source.appVersion, "RuntimeStatus.appVersion");
  string(source.kernelStatus, "RuntimeStatus.kernelStatus");
  boolean(source.providerReady, "RuntimeStatus.providerReady");
  string(source.workspaceRoot, "RuntimeStatus.workspaceRoot");
  stringArray(source.orchestrationModes, "RuntimeStatus.orchestrationModes");
  stringArray(source.registeredTools, "RuntimeStatus.registeredTools");
  runBudgets(source.agentRunBudgets);
  return source as NativeRuntimeStatus;
}
