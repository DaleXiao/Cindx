export type AgentEffort = "fast" | "default" | "high" | "xhigh";

export type AgentRunBudget = {
  maxDurationMs: number;
  maxModelCalls: number;
  maxToolCalls: number;
};

export type AgentRunBudgets = Record<AgentEffort, AgentRunBudget>;

export type OptimisticRunBudgetPatch = {
  maxTurns: number;
  runBudgetMs: number;
  runModelCallBudget: number;
  runToolCallBudget: number;
};

export function optimisticRunBudgetPatch(
  budgets: AgentRunBudgets | null | undefined,
  effort: AgentEffort
): OptimisticRunBudgetPatch {
  const budget = budgets?.[effort];
  return {
    maxTurns: budget?.maxModelCalls ?? 0,
    runBudgetMs: budget?.maxDurationMs ?? 0,
    runModelCallBudget: budget?.maxModelCalls ?? 0,
    runToolCallBudget: budget?.maxToolCalls ?? 0
  };
}
