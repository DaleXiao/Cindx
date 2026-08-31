import type { AgentEffort } from "./agentRunBudgetModel";
import type { ApprovalPolicy } from "./tauri";

/**
 * Plan mode (plan-then-confirm) is a Settings opt-in (the default-off
 * plan-first toggle) available only on the High/Xhigh tiers. Fast and Default
 * never honor it, and a stale request on those tiers is normalized away before
 * it reaches the backend.
 */
export function planModeAvailable(effort: AgentEffort): boolean {
  return effort === "high" || effort === "xhigh";
}

/** The plan-mode flag sent with a run or queued message. */
export function planModeForSubmission(effort: AgentEffort, requested: boolean): boolean {
  return requested && planModeAvailable(effort);
}

export type PlanConfirmationDecision = "approve" | "discard" | "cancel";

/** Who resolved a plan confirmation; recorded on the plan_resolved event. */
export type PlanResolvedBy = "local-user" | "auto-timeout";

/** The idle window before a plan card approves itself. */
export const PLAN_AUTO_APPROVAL_WINDOW_MS = 30_000;

/**
 * The idle auto-approve timer may only run under the strict approval policy:
 * there, every effect still prompts individually, so the timer merely unblocks
 * the run. Under session/all policies the run trends fully automatic, so the
 * plan gate must stay an explicit user decision (the backend rejects timeout
 * approvals under those policies too).
 */
export function planAutoApprovalAllowed(approvalPolicy: ApprovalPolicy): boolean {
  return approvalPolicy === "strict";
}

/**
 * Whether the auto-approval countdown may advance right now. Any sign of
 * reading (engagement), a hidden document, or an unfocused window pauses it.
 */
export function planAutoApprovalActive(state: {
  engaged: boolean;
  documentVisible: boolean;
  windowFocused: boolean;
}): boolean {
  return !state.engaged && state.documentVisible && state.windowFocused;
}

/**
 * Advance the auto-approval countdown by one tick. The countdown accumulates
 * *active* time only: while the user reads, the document is hidden, or the
 * window is unfocused the remaining time is unchanged, so leaving and coming
 * back can never cause an immediate approval (the previous wall-clock model
 * did). A failed resolution resets the countdown to the full window.
 */
export function advancePlanAutoApproval(
  remainingMs: number,
  tickMs: number,
  active: boolean
): number {
  if (!active) return remainingMs;
  return Math.max(0, remainingMs - tickMs);
}
