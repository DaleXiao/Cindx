import assert from "node:assert/strict";
import test from "node:test";
import { createInitialPhase4State } from "../src/browserPreviewFallbackState.ts";
import {
  APPROVAL_POLICY_OPTIONS,
  approvalPolicyIsAutomatic,
  normalizeApprovalPolicy,
  approvalBlockedBySessionBusy
} from "../src/approvalPolicyModel.ts";

test("the approval policy dropdown renders exactly the three tiers", () => {
  assert.deepEqual(
    APPROVAL_POLICY_OPTIONS.map((option) => option.value),
    ["strict", "session", "all"]
  );
  assert.deepEqual(
    APPROVAL_POLICY_OPTIONS.map((option) => option.label),
    ["Strict", "Approve in session", "Approve all"]
  );
});

test("approval policy normalization fails closed to strict", () => {
  assert.equal(normalizeApprovalPolicy("strict"), "strict");
  assert.equal(normalizeApprovalPolicy("session"), "session");
  assert.equal(normalizeApprovalPolicy("all"), "all");
  for (const value of [undefined, null, "", "always", "SESSION", "Auto"]) {
    assert.equal(normalizeApprovalPolicy(value), "strict");
  }
});

test("only session and all policies auto-approve; strict always prompts", () => {
  assert.equal(approvalPolicyIsAutomatic("strict"), false);
  assert.equal(approvalPolicyIsAutomatic("session"), true);
  assert.equal(approvalPolicyIsAutomatic("all"), true);
});

test("the approval policy defaults to strict", () => {
  assert.equal(createInitialPhase4State().provider.approvalPolicy, "strict");
});

test("subagent approvals stay actionable while the parent run is busy", () => {
  // A normal approval waits for an idle session...
  assert.equal(approvalBlockedBySessionBusy(false, true), true);
  // ...but a write subagent's approval parks its parent run, so blocking it
  // on the session busy state would deadlock the decision (R1).
  assert.equal(approvalBlockedBySessionBusy(true, true), false);
  assert.equal(approvalBlockedBySessionBusy(false, false), false);
  assert.equal(approvalBlockedBySessionBusy(true, false), false);
});
