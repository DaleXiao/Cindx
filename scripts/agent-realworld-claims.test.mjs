import assert from "node:assert/strict";
import test from "node:test";

import {
  CLAIM_DIMENSIONS,
  CLAIM_STATES,
  evaluateAgentClaimStates,
  evaluateClaimState
} from "./agent-realworld-claims.mjs";
import { evaluateCurrentShippingClaims } from "./agent-realworld-current-claims.mjs";

const hashes = {
  candidate: "a".repeat(64),
  stable: "b".repeat(64),
  pair: "c".repeat(64),
  input: "d".repeat(64),
  teacher: "e".repeat(64)
};

function strategy(role, dimension) {
  const candidate = role === "candidate";
  return {
    execution_mode: dimension === "adaptive_direct" ? "direct" : "workflow",
    workflow_profile_exercised: dimension !== "adaptive_direct",
    profile_source: "evaluation_frozen_profile",
    profile_id: candidate ? "candidate-profile" : "stable-profile",
    profile_sha256: candidate ? hashes.candidate : hashes.stable,
    profile_pair_sha256: hashes.pair,
    stable_profile_id: candidate ? "stable-profile" : null,
    stable_profile_sha256: candidate ? hashes.stable : null,
    learned_artifact_sha256: candidate ? hashes.candidate : hashes.stable,
    learned_method: dimension === "distillation" ? "pro_to_auto_distillation" : "gepa_reflective_paired",
    teacher_profile_id: dimension === "distillation" ? "pro-teacher" : null,
    teacher_profile_sha256: dimension === "distillation" ? hashes.teacher : null
  };
}

function run(role, dimension, replicate = 1) {
  const candidate = role === "candidate";
  return {
    case_id: "case-a",
    replicate,
    treatment: candidate ? `${dimension}_candidate` : `${dimension}_stable`,
    input_sha256: hashes.input,
    product_mechanism_exercised: true,
    completed: true,
    terminal_status: "completed",
    configured_models: {
      conductor: "model-a",
      executor: "model-b"
    },
    resolved_budget: {
      max_duration_ms: 10_000,
      max_model_calls: 4,
      max_tool_calls: 8
    },
    claim_receipts: {
      [dimension]: { exercised: candidate }
    },
    strategy_receipt: strategy(role, dimension),
    metrics: {
      latency_ms: 100,
      total_tokens: 200
    },
    verification: {
      quality_passed: true
    }
  };
}

function claim(dimension, expectedPairs = 1) {
  return {
    expected_pairs: expectedPairs,
    candidate: {
      treatment: `${dimension}_candidate`,
      profile: {
        id: "candidate-profile",
        sha256: hashes.candidate,
        pair_sha256: hashes.pair,
        source: "evaluation_frozen_profile",
        learned_method:
          dimension === "distillation"
            ? "pro_to_auto_distillation"
            : "gepa_reflective_paired",
        exact_parent_profile_id: "stable-profile",
        exact_parent_profile_sha256: hashes.stable,
        teacher_profile_id: dimension === "distillation" ? "pro-teacher" : undefined,
        teacher_profile_sha256: dimension === "distillation" ? hashes.teacher : undefined
      }
    },
    stable: {
      treatment: `${dimension}_stable`,
      profile: {
        id: "stable-profile",
        sha256: hashes.stable,
        pair_sha256: hashes.pair,
        source: "evaluation_frozen_profile",
        learned_method:
          dimension === "distillation"
            ? "pro_to_auto_distillation"
            : "gepa_reflective_paired"
      }
    }
  };
}

function fixture(dimension, expectedPairs = 1) {
  const runs = [];
  for (let replicate = 1; replicate <= expectedPairs; replicate += 1) {
    runs.push(run("stable", dimension, replicate));
    runs.push(run("candidate", dimension, replicate));
  }
  return { runs, claim: claim(dimension, expectedPairs) };
}

const currentContract = {
  minimum_quality_delta: 0,
  minimum_completion_delta: 0,
  minimum_any_quality_improvement_runs: 1,
  maximum_setup_failures: 0,
  maximum_safety_violations: 0,
  candidates: {
    auto: {
      maximum_median_latency_ratio: 3,
      maximum_total_token_ratio: 4
    }
  }
};

function currentRun({
  treatment,
  caseId,
  mode = "direct",
  workflowProfileExercised = false,
  quality = true,
  completed = true,
  latency = 100,
  tokens = 200,
  strategyReceipt = true,
  routing = mode
}) {
  return {
    case_id: caseId,
    replicate: 1,
    treatment,
    input_sha256: hashes.input,
    product_mechanism_exercised: true,
    completed,
    terminal_status: completed ? "completed" : "timed_out",
    evidence_error: null,
    configured_models: ["model-a"],
    resolved_budget: { max_duration_ms: 10_000, max_model_calls: 4 },
    setup_failure: null,
    strategy_receipt: strategyReceipt
      ? {
          execution_mode: mode,
          workflow_profile_exercised: workflowProfileExercised,
          routing_signature_sha256: `${routing.charCodeAt(0).toString(16)}`.repeat(64).slice(0, 64),
          profile_source: "built_in_seed",
          profile_id: "auto-seed",
          profile_sha256: hashes.candidate
        }
      : null,
    model_receipts: [{ receipt_status: "observed" }],
    metrics: {
      model_responses: 1,
      latency_ms: latency,
      total_tokens: tokens
    },
    verification: {
      quality_passed: quality,
      safety_violations: 0
    }
  };
}

function currentPair(caseId, mode, overrides = {}) {
  return [
    currentRun({
      treatment: "grounded_direct",
      caseId,
      routing: "stable",
      ...overrides.stable
    }),
    currentRun({
      treatment: "auto",
      caseId,
      mode,
      workflowProfileExercised: mode === "workflow",
      routing: mode === "direct" ? "stable" : "workflow",
      ...overrides.auto
    })
  ];
}

test("reports all four absent claim dimensions as not exercised", () => {
  const result = evaluateAgentClaimStates();
  assert.deepEqual(Object.keys(result), CLAIM_DIMENSIONS);
  for (const dimension of CLAIM_DIMENSIONS) {
    assert.equal(result[dimension].state, CLAIM_STATES.NOT_EXERCISED);
    assert.equal(result[dimension].reason, "claim_not_configured");
  }
});

test("complete equivalent matched pairs with zero outcome delta are neutral", () => {
  for (const dimension of CLAIM_DIMENSIONS) {
    const value = fixture(dimension, 2);
    const result = evaluateClaimState({ dimension, ...value });
    assert.equal(result.state, CLAIM_STATES.NEUTRAL, dimension);
    assert.equal(result.evidence_complete, true, dimension);
    assert.equal(result.denominator, 2, dimension);
    assert.equal(result.quality_delta_runs, 0, dimension);
    assert.equal(result.completion_delta_runs, 0, dimension);
  }
});

test("missing actual behavior is not exercised rather than neutral", () => {
  const value = fixture("workflow");
  value.runs.find((item) => item.treatment.endsWith("candidate")).claim_receipts.workflow.exercised = false;
  const result = evaluateClaimState({ dimension: "workflow", ...value });
  assert.equal(result.state, CLAIM_STATES.NOT_EXERCISED);
  assert.equal(result.reason, "candidate_behavior_not_exercised");
});

test("missing exact parent evidence is not exercised", () => {
  for (const dimension of ["learned_profile", "distillation"]) {
    const value = fixture(dimension);
    const candidate = value.runs.find((item) => item.treatment.endsWith("candidate"));
    candidate.strategy_receipt.stable_profile_sha256 = null;
    const result = evaluateClaimState({ dimension, ...value });
    assert.equal(result.state, CLAIM_STATES.NOT_EXERCISED, dimension);
    assert.equal(result.reason, "exact_stable_parent_not_exercised", dimension);
  }
});

test("a claimed but different stable parent fails closed", () => {
  const value = fixture("learned_profile");
  const candidate = value.runs.find((item) => item.treatment.endsWith("candidate"));
  candidate.strategy_receipt.stable_profile_id = "different-parent";
  const result = evaluateClaimState({ dimension: "learned_profile", ...value });
  assert.equal(result.state, CLAIM_STATES.INVALID_EVIDENCE);
  assert.match(result.errors.join("\n"), /exercised parent id/);
});

test("budget, model, and profile receipt drift each fail closed", () => {
  const mutations = [
    {
      label: "budget",
      change(candidate) {
        candidate.resolved_budget.max_model_calls = 5;
      },
      pattern: /budget receipt mismatch/
    },
    {
      label: "model",
      change(candidate) {
        candidate.configured_models.executor = "other-model";
      },
      pattern: /model binding receipt mismatch/
    },
    {
      label: "profile",
      change(candidate) {
        candidate.strategy_receipt.profile_sha256 = "f".repeat(64);
      },
      pattern: /profile hash mismatch/
    }
  ];
  for (const mutation of mutations) {
    const value = fixture("workflow");
    const candidate = value.runs.find((item) => item.treatment.endsWith("candidate"));
    mutation.change(candidate);
    const result = evaluateClaimState({ dimension: "workflow", ...value });
    assert.equal(result.state, CLAIM_STATES.INVALID_EVIDENCE, mutation.label);
    assert.match(result.errors.join("\n"), mutation.pattern, mutation.label);
  }
});

test("candidate failures and timeouts remain in the matched denominator", () => {
  const value = fixture("learned_profile", 2);
  const candidate = value.runs.find(
    (item) => item.treatment.endsWith("candidate") && item.replicate === 2
  );
  candidate.completed = false;
  candidate.terminal_status = "timed_out";
  candidate.verification.quality_passed = false;
  candidate.metrics.latency_ms = 600_000;
  candidate.metrics.total_tokens = 17;

  const result = evaluateClaimState({ dimension: "learned_profile", ...value });
  assert.equal(result.state, CLAIM_STATES.REGRESSED);
  assert.equal(result.denominator, 2);
  assert.equal(result.matched_pairs, 2);
  assert.equal(result.candidate_failed_runs, 1);
  assert.equal(result.candidate_timeouts, 1);
  assert.equal(result.quality_delta_runs, -1);
  assert.equal(result.completion_delta_runs, -1);
  assert.equal(result.candidate_tokens_total, 217);
});

test("a stable-side failure is retained as an observed candidate improvement", () => {
  const value = fixture("adaptive_direct");
  const stable = value.runs.find((item) => item.treatment.endsWith("stable"));
  stable.completed = false;
  stable.terminal_status = "failed";
  stable.verification.quality_passed = false;
  const result = evaluateClaimState({ dimension: "adaptive_direct", ...value });
  assert.equal(result.state, CLAIM_STATES.IMPROVED);
  assert.equal(result.denominator, 1);
  assert.equal(result.stable_failed_runs, 1);
  assert.equal(result.quality_delta_runs, 1);
  assert.equal(result.completion_delta_runs, 1);
});

test("missing, extra, or duplicate matched cells are invalid evidence", () => {
  const missing = fixture("workflow", 2);
  missing.runs.pop();
  assert.equal(
    evaluateClaimState({ dimension: "workflow", ...missing }).state,
    CLAIM_STATES.INVALID_EVIDENCE
  );

  const duplicate = fixture("workflow");
  duplicate.runs.push(structuredClone(duplicate.runs[1]));
  const result = evaluateClaimState({ dimension: "workflow", ...duplicate });
  assert.equal(result.state, CLAIM_STATES.INVALID_EVIDENCE);
  assert.match(result.errors.join("\n"), /duplicate matched cell/);
});

test("current mixed routing evaluates adaptive-direct and workflow on matched subsets", () => {
  const runs = [
    ...currentPair("direct-case", "direct"),
    ...currentPair("workflow-case", "workflow", {
      stable: { quality: false }
    })
  ];
  const states = evaluateCurrentShippingClaims(runs, currentContract);
  assert.equal(states.adaptive_direct.state, CLAIM_STATES.NEUTRAL);
  assert.equal(states.adaptive_direct.denominator, 1);
  assert.equal(states.workflow.state, CLAIM_STATES.IMPROVED);
  assert.equal(states.workflow.denominator, 1);
  assert.equal(states.workflow.claim_contract_gates.quality_improvement, true);
});

test("current claims require quality uplift and enforce frozen resource limits", () => {
  const completionOnly = currentPair("completion-only", "direct", {
    stable: { quality: false, completed: false },
    auto: { quality: false, routing: "changed" }
  });
  const completionState = evaluateCurrentShippingClaims(
    completionOnly,
    currentContract
  ).adaptive_direct;
  assert.equal(completionState.state, CLAIM_STATES.NEUTRAL);
  assert.equal(completionState.reason, "quality_improvement_threshold_not_met");

  const resourceRegression = currentPair("resource-regression", "workflow", {
    stable: { quality: false, latency: 10, tokens: 10 },
    auto: { quality: true, latency: 100, tokens: 100 }
  });
  const resourceState = evaluateCurrentShippingClaims(
    resourceRegression,
    currentContract
  ).workflow;
  assert.equal(resourceState.state, CLAIM_STATES.MIXED);
  assert.equal(resourceState.reason, "resource_limit_exceeded");
  assert.equal(resourceState.claim_contract_gates.latency_within_limit, false);
  assert.equal(resourceState.claim_contract_gates.tokens_within_limit, false);
});

test("an unclassified early Auto timeout remains in both mechanism denominators", () => {
  const runs = currentPair("early-timeout", "direct", {
    auto: { completed: false, strategyReceipt: false }
  });
  const states = evaluateCurrentShippingClaims(runs, currentContract);
  for (const dimension of ["adaptive_direct", "workflow"]) {
    assert.equal(states[dimension].state, CLAIM_STATES.INVALID_EVIDENCE);
    assert.equal(states[dimension].denominator, 1);
    assert.equal(states[dimension].candidate_timeouts, 1);
  }
});

test("workflow without a profile-exercised receipt cannot claim uplift", () => {
  const runs = currentPair("workflow-no-profile", "workflow", {
    auto: { workflowProfileExercised: false }
  });
  const workflow = evaluateCurrentShippingClaims(runs, currentContract).workflow;
  assert.equal(workflow.state, CLAIM_STATES.NOT_EXERCISED);
  assert.equal(workflow.denominator, 1);
  assert.equal(workflow.reason, "candidate_behavior_not_exercised");
});
