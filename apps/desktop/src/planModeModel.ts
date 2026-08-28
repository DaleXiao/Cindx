import type { AgentEffort } from "./agentRunBudgetModel";

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
