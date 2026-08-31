import assert from "node:assert/strict";
import test from "node:test";
import { createInitialPhase4State } from "../src/browserPreviewFallbackState.ts";
import {
  planAutoApprovalActive,
  planAutoApprovalAllowed,
  planAutoApproveRemainingMs,
  planModeAvailable,
  planModeForSubmission,
  PLAN_AUTO_APPROVAL_WINDOW_MS
} from "../src/planModeModel.ts";

test("plan mode is available only for high and xhigh effort", () => {
  assert.equal(planModeAvailable("fast"), false);
  assert.equal(planModeAvailable("default"), false);
  assert.equal(planModeAvailable("high"), true);
  assert.equal(planModeAvailable("xhigh"), true);
});

test("the plan-first setting defaults off", () => {
  assert.equal(createInitialPhase4State().provider.planFirstEnabled, false);
  for (const effort of ["fast", "default", "high", "xhigh"] as const) {
    assert.equal(planModeForSubmission(effort, false), false);
  }
});

test("an enabled plan-first setting sends planMode only on gated tiers", () => {
  assert.equal(planModeForSubmission("high", true), true);
  assert.equal(planModeForSubmission("xhigh", true), true);
  // Fast and Default never honor the setting.
  assert.equal(planModeForSubmission("fast", true), false);
  assert.equal(planModeForSubmission("default", true), false);
});

test("plan auto-approval is allowed only under the strict approval policy", () => {
  assert.equal(planAutoApprovalAllowed("strict"), true);
  // Under session/all policies the run trends fully automatic, so the plan
  // gate must stay an explicit user decision.
  assert.equal(planAutoApprovalAllowed("session"), false);
  assert.equal(planAutoApprovalAllowed("all"), false);
});

test("plan auto-approval pauses while reading, hidden, or unfocused", () => {
  const idle = { engaged: false, documentVisible: true, windowFocused: true };
  assert.equal(planAutoApprovalActive(idle), true);
  assert.equal(planAutoApprovalActive({ ...idle, engaged: true }), false);
  assert.equal(planAutoApprovalActive({ ...idle, documentVisible: false }), false);
  assert.equal(planAutoApprovalActive({ ...idle, windowFocused: false }), false);
});

test("the plan auto-approval window counts down from the proposal time", () => {
  const proposedAt = 1_000_000;
  assert.equal(
    planAutoApproveRemainingMs(proposedAt, proposedAt, proposedAt),
    PLAN_AUTO_APPROVAL_WINDOW_MS
  );
  assert.equal(
    planAutoApproveRemainingMs(proposedAt, proposedAt, proposedAt + 12_000),
    PLAN_AUTO_APPROVAL_WINDOW_MS - 12_000
  );
  assert.equal(
    planAutoApproveRemainingMs(proposedAt, proposedAt, proposedAt + 30_000),
    0
  );
  assert.equal(
    planAutoApproveRemainingMs(proposedAt, proposedAt, proposedAt + 999_999),
    0,
    "the window never goes negative"
  );
});

test("a failed resolution restarts the full plan auto-approval window", () => {
  const proposedAt = 1_000_000;
  const failureResetAt = proposedAt + 29_000;
  assert.equal(
    planAutoApproveRemainingMs(proposedAt, failureResetAt, failureResetAt + 1_000),
    PLAN_AUTO_APPROVAL_WINDOW_MS - 1_000,
    "the window restarts from the failure reset, not the proposal time"
  );
});
