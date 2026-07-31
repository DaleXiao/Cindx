export type RagOperationKind = "index" | "search" | "answer";

export type RagOperationProgress = {
  operationId: string;
  operationKind: RagOperationKind;
  stage: string;
  completedSteps: number;
  totalSteps: number;
  detail: string;
  status: "running" | "completed" | "cancelled" | "failed";
};

const TERMINAL_RAG_OPERATION_STATUSES = new Set(["completed", "cancelled", "failed"]);

export type RagCancelOutcome = {
  accepted: boolean;
  signalError: string | null;
};

export type RagCancelAttempt = {
  operationId: string;
  outcome: Promise<RagCancelOutcome>;
  current: RagCancelOutcome | null;
  settle: (outcome: RagCancelOutcome) => void;
};

const NO_RAG_CANCEL: RagCancelOutcome = { accepted: false, signalError: null };

export function createRagOperationId(operationKind: RagOperationProgress["operationKind"]) {
  const uniquePart =
    globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `rag-${operationKind}-${uniquePart}`;
}

export function acceptRagOperationProgress(
  activeOperationId: string | null,
  current: RagOperationProgress | null,
  incoming: RagOperationProgress
) {
  if (incoming.operationId !== activeOperationId) return current;
  if (current?.operationId !== incoming.operationId) return incoming;
  if (TERMINAL_RAG_OPERATION_STATUSES.has(current.status)) return current;
  if (incoming.completedSteps < current.completedSteps) return current;
  return incoming;
}

export function shouldApplyRagOperationResult(
  activeOperationId: string | null,
  completedOperationId: string,
  cancelOutcome: RagCancelOutcome
) {
  return activeOperationId === completedOperationId && !cancelOutcome.accepted;
}

export function createRagCancelAttempt(operationId: string): RagCancelAttempt {
  let resolveOutcome: (outcome: RagCancelOutcome) => void = () => {};
  const attempt: RagCancelAttempt = {
    operationId,
    outcome: new Promise<RagCancelOutcome>((resolve) => {
      resolveOutcome = resolve;
    }),
    current: null,
    settle: (outcome) => {
      if (attempt.current) return;
      attempt.current = outcome;
      resolveOutcome(outcome);
    }
  };
  return attempt;
}

export function waitForRagCancelOutcome(
  operationId: string,
  attempt: RagCancelAttempt | null
): Promise<RagCancelOutcome> {
  if (attempt?.operationId !== operationId) return Promise.resolve(NO_RAG_CANCEL);
  return attempt.current ? Promise.resolve(attempt.current) : attempt.outcome;
}

export function clearRagCancelAttemptIfCurrent(
  current: RagCancelAttempt | null,
  settled: RagCancelAttempt
) {
  return current === settled ? null : current;
}
