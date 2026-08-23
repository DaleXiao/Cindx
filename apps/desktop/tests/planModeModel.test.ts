import assert from "node:assert/strict";
import test from "node:test";
import {
  planModeAvailable,
  planModeForSubmission
} from "../src/planModeModel.ts";

test("the plan-mode entry is visible only for high and xhigh effort", () => {
  assert.equal(planModeAvailable("fast"), false);
  assert.equal(planModeAvailable("default"), false);
  assert.equal(planModeAvailable("high"), true);
  assert.equal(planModeAvailable("xhigh"), true);
});

test("a requested plan mode reaches the backend only on gated tiers", () => {
  assert.equal(planModeForSubmission("high", true), true);
  assert.equal(planModeForSubmission("xhigh", true), true);
  assert.equal(planModeForSubmission("fast", true), false);
  assert.equal(planModeForSubmission("default", true), false);
  assert.equal(planModeForSubmission("high", false), false);
});

test("plan mode defaults off and never turns itself on", () => {
  for (const effort of ["fast", "default", "high", "xhigh"] as const) {
    assert.equal(planModeForSubmission(effort, false), false);
  }
});
