import assert from "node:assert/strict";
import test from "node:test";
import {
  acceptRagOperationProgress,
  clearRagCancelAttemptIfCurrent,
  createRagCancelAttempt,
  shouldApplyRagOperationResult,
  waitForRagCancelOutcome,
  type RagOperationProgress
} from "../src/ragOperationModel.ts";

function progress(
  operationId: string,
  completedSteps: number,
  status: RagOperationProgress["status"] = "running"
): RagOperationProgress {
  return {
    operationId,
    operationKind: "search",
    stage: status,
    completedSteps,
    totalSteps: 4,
    detail: status,
    status
  };
}

test("accepts progress only for the active RAG operation", () => {
  const current = progress("rag-search-current", 1);
  assert.equal(
    acceptRagOperationProgress("rag-search-current", current, progress("rag-search-old", 3)),
    current
  );

  const next = progress("rag-search-current", 2);
  assert.equal(acceptRagOperationProgress("rag-search-current", current, next), next);
});

test("does not regress progress or overwrite a terminal operation", () => {
  const current = progress("rag-answer-current", 2);
  assert.equal(
    acceptRagOperationProgress("rag-answer-current", current, progress("rag-answer-current", 1)),
    current
  );

  const completed = progress("rag-answer-current", 4, "completed");
  assert.equal(
    acceptRagOperationProgress("rag-answer-current", completed, progress("rag-answer-current", 4)),
    completed
  );
});

test("keeps prior results when a completed operation is stale or cancellation was accepted", () => {
  const notCancelled = { accepted: false, signalError: null };
  const cancelled = { accepted: true, signalError: null };
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", notCancelled), true);
  assert.equal(shouldApplyRagOperationResult("rag-new", "rag-old", notCancelled), false);
  assert.equal(
    shouldApplyRagOperationResult("rag-current", "rag-current", cancelled),
    false
  );
});

test("holds an early success until cancel rejection makes it safe to apply", async () => {
  const attempt = createRagCancelAttempt("rag-current");
  let decided = false;
  const outcomePromise = waitForRagCancelOutcome("rag-current", attempt).then((outcome) => {
    decided = true;
    return outcome;
  });
  await Promise.resolve();
  assert.equal(decided, false);

  attempt.settle({ accepted: false, signalError: null });
  const outcome = await outcomePromise;
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", outcome), true);
});

test("cancel acceptance suppresses results in either completion ordering", async () => {
  const cancelFirst = createRagCancelAttempt("rag-current");
  cancelFirst.settle({ accepted: true, signalError: null });
  const earlyOutcome = await waitForRagCancelOutcome("rag-current", cancelFirst);
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", earlyOutcome), false);

  const resultFirst = createRagCancelAttempt("rag-current");
  const lateOutcomePromise = waitForRagCancelOutcome("rag-current", resultFirst);
  resultFirst.settle({ accepted: true, signalError: null });
  const lateOutcome = await lateOutcomePromise;
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", lateOutcome), false);
});

test("cancel rejection and signaling failure keep a terminal result applicable", async () => {
  const rejected = createRagCancelAttempt("rag-current");
  rejected.settle({ accepted: false, signalError: null });
  assert.equal(
    shouldApplyRagOperationResult(
      "rag-current",
      "rag-current",
      await waitForRagCancelOutcome("rag-current", rejected)
    ),
    true
  );

  const failed = createRagCancelAttempt("rag-current");
  failed.settle({ accepted: false, signalError: "cancel IPC failed" });
  const failedOutcome = await waitForRagCancelOutcome("rag-current", failed);
  assert.equal(failedOutcome.signalError, "cancel IPC failed");
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", failedOutcome), true);
});

test("a rejected cancel can be released, retried, and accepted without a stale clear", async () => {
  const rejected = createRagCancelAttempt("rag-current");
  rejected.settle({ accepted: false, signalError: null });
  let current = clearRagCancelAttemptIfCurrent(rejected, rejected);
  assert.equal(current, null);

  const retry = createRagCancelAttempt("rag-current");
  current = retry;
  assert.equal(clearRagCancelAttemptIfCurrent(current, rejected), retry);
  retry.settle({ accepted: true, signalError: null });
  const outcome = await waitForRagCancelOutcome("rag-current", retry);
  assert.equal(outcome.accepted, true);
  assert.equal(shouldApplyRagOperationResult("rag-current", "rag-current", outcome), false);
});
