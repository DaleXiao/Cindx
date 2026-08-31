import assert from "node:assert/strict";
import test from "node:test";
import { createInitialPhase4State } from "../src/browserPreviewFallbackState.ts";
import {
  advancePlanAutoApproval,
  planAutoApprovalActive,
  planAutoApprovalAllowed,
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

test("the plan auto-approval countdown accumulates active time only", () => {
  let remaining = PLAN_AUTO_APPROVAL_WINDOW_MS;
  for (let tick = 0; tick < 12; tick += 1) {
    remaining = advancePlanAutoApproval(remaining, 1000, true);
  }
  assert.equal(remaining, PLAN_AUTO_APPROVAL_WINDOW_MS - 12_000);

  // Hidden or unfocused ticks do not count, so leaving for 30s and returning
  // can never cause an immediate approval.
  for (let tick = 0; tick < 30; tick += 1) {
    remaining = advancePlanAutoApproval(remaining, 1000, false);
  }
  assert.equal(remaining, PLAN_AUTO_APPROVAL_WINDOW_MS - 12_000);

  for (let tick = 0; tick < 18; tick += 1) {
    remaining = advancePlanAutoApproval(remaining, 1000, true);
  }
  assert.equal(remaining, 0);
  assert.equal(
    advancePlanAutoApproval(0, 1000, true),
    0,
    "the countdown never goes negative"
  );
});

test("a failed resolution restarts the full plan auto-approval window", () => {
  // The card resets remaining to the full window on a failed resolution; the
  // model must treat a fresh full window as a complete countdown.
  const reset = PLAN_AUTO_APPROVAL_WINDOW_MS;
  assert.equal(advancePlanAutoApproval(reset, 1000, true), PLAN_AUTO_APPROVAL_WINDOW_MS - 1000);
});
