import type { AgentEffort } from "./agentRunBudgetModel";

/**
 * Plan mode (plan-then-confirm) is an explicit opt-in available only on the
 * High/Xhigh tiers. Fast and Default never expose the entry point, and a stale
 * request on those tiers is normalized away before it reaches the backend.
 */
export function planModeAvailable(effort: AgentEffort): boolean {
  return effort === "high" || effort === "xhigh";
}

/** The plan-mode flag sent with a run or queued message. */
export function planModeForSubmission(effort: AgentEffort, requested: boolean): boolean {
  return requested && planModeAvailable(effort);
}

export type PlanConfirmationDecision = "approve" | "discard" | "cancel";
