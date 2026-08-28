import assert from "node:assert/strict";
import test from "node:test";
import { createInitialPhase4State } from "../src/browserPreviewFallbackState.ts";
import {
  planModeAvailable,
  planModeForSubmission
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
