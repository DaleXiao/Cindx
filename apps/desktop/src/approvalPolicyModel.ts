import type { ApprovalPolicy } from "./tauri";

export type ApprovalPolicyOption = {
  value: ApprovalPolicy;
  label: string;
};

/**
 * The three-tier agent approval policy offered in Settings → Permissions,
 * in display order. Strict is the default and keeps the historical behavior
 * of prompting for every permission request.
 */
export const APPROVAL_POLICY_OPTIONS: readonly ApprovalPolicyOption[] = [
  { value: "strict", label: "Strict" },
  { value: "session", label: "Approve in session" },
  { value: "all", label: "Approve all" }
];

/**
 * Fail-closed normalization: only the known policies pass through, every
 * other value (missing, unknown, or malformed) falls back to Strict so the
 * run keeps prompting the user.
 */
export function normalizeApprovalPolicy(value: string | null | undefined): ApprovalPolicy {
  if (value === "session" || value === "all") return value;
  return "strict";
}

/** True when the policy auto-approves eligible (non-destructive) requests. */
export function approvalPolicyIsAutomatic(policy: ApprovalPolicy): boolean {
  return policy === "session" || policy === "all";
}

/**
 * A write subagent's approval arrives while its parent run keeps the session
 * busy — the parent is parked waiting for exactly this decision. Blocking the
 * buttons on the run finishing would deadlock the flow, so subagent approvals
 * stay actionable while every other approval waits for an idle session (R1).
 */
export function approvalBlockedBySessionBusy(subagent: boolean, sessionBusy: boolean): boolean {
  return sessionBusy && !subagent;
}
