import assert from "node:assert/strict";
import test from "node:test";
import {
  optimisticRunBudgetPatch,
  type AgentRunBudgets
} from "../src/agentRunBudgetModel.ts";

const budgets: AgentRunBudgets = {
  fast: { maxDurationMs: 11, maxModelCalls: 12, maxToolCalls: 13 },
  auto: { maxDurationMs: 21, maxModelCalls: 22, maxToolCalls: 23 },
  pro: { maxDurationMs: 31, maxModelCalls: 32, maxToolCalls: 33 }
};

test("optimistic run state uses every runtime-provided budget field", () => {
  assert.deepEqual(optimisticRunBudgetPatch(budgets, "auto"), {
    maxTurns: 22,
    runBudgetMs: 21,
    runModelCallBudget: 22,
    runToolCallBudget: 23
  });
});

test("missing runtime budget does not invent a frontend fallback", () => {
  const emptyPatch = {
    maxTurns: 0,
    runBudgetMs: 0,
    runModelCallBudget: 0,
    runToolCallBudget: 0
  };
  assert.deepEqual(optimisticRunBudgetPatch(null, "auto"), emptyPatch);
  assert.deepEqual(optimisticRunBudgetPatch(undefined, "auto"), emptyPatch);
});
