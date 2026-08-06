import {
  CLAIM_STATES,
  evaluateAgentClaimStates
} from "./agent-realworld-claims.mjs";

function profileBinding(run) {
  const receipt = run?.strategy_receipt;
  if (!receipt?.profile_id || !receipt?.profile_sha256) return null;
  return {
    id: receipt.profile_id,
    sha256: receipt.profile_sha256,
    source: receipt.profile_source
  };
}

function pairKey(run) {
  return `${run.case_id}/r${run.replicate}`;
}

function matchedProductRuns(runs) {
  return {
    grounded: runs.filter((run) => run.treatment === "grounded_direct"),
    auto: runs.filter((run) => run.treatment === "auto")
  };
}

function claimForMode(runs, mode) {
  const { grounded, auto } = matchedProductRuns(runs);
  const expectedMode = mode === "adaptive_direct" ? "direct" : "workflow";
  const selectedAuto = auto.filter(
    (run) => run.strategy_receipt?.execution_mode === expectedMode
  );
  if (selectedAuto.length === 0) return null;
  const pairKeys = selectedAuto.map(pairKey).sort();
  const groundedByKey = new Map(grounded.map((run) => [pairKey(run), run]));
  const stable = groundedByKey.get(pairKeys[0]);
  return {
    expected_pairs: pairKeys.length,
    pair_keys: pairKeys,
    require_provider_evidence: true,
    candidate: {
      treatment: "auto",
      profile: profileBinding(selectedAuto[0])
    },
    stable: {
      treatment: "grounded_direct",
      profile: profileBinding(stable)
    }
  };
}

function pairedRuns(runs, claim) {
  if (!claim?.pair_keys) return [];
  const { grounded, auto } = matchedProductRuns(runs);
  const candidateByKey = new Map(auto.map((run) => [pairKey(run), run]));
  const stableByKey = new Map(grounded.map((run) => [pairKey(run), run]));
  return claim.pair_keys
    .map((key) => ({ candidate: candidateByKey.get(key), stable: stableByKey.get(key) }))
    .filter((pair) => pair.candidate && pair.stable);
}

function median(values) {
  if (values.length === 0) return null;
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.max(0, Math.ceil(ordered.length / 2) - 1)];
}

function boundedRatio(candidate, stable) {
  if (stable === 0) return candidate === 0 ? 1 : null;
  return candidate / stable;
}

function sum(values) {
  return values.reduce((total, value) => total + value, 0);
}

function applyClaimContract(state, runs, claim, contract) {
  if (!state.evidence_complete || !claim || !contract) return state;
  const pairs = pairedRuns(runs, claim);
  const candidateRuns = pairs.map((pair) => pair.candidate);
  const stableRuns = pairs.map((pair) => pair.stable);
  const limits = contract.candidates.auto;
  const candidateLatency = median(candidateRuns.map((run) => run.metrics.latency_ms));
  const stableLatency = median(stableRuns.map((run) => run.metrics.latency_ms));
  const latencyRatio = boundedRatio(candidateLatency, stableLatency);
  const tokenRatio = boundedRatio(
    sum(candidateRuns.map((run) => run.metrics.total_tokens)),
    sum(stableRuns.map((run) => run.metrics.total_tokens))
  );
  const setupFailures = [...candidateRuns, ...stableRuns].filter(
    (run) => run.setup_failure != null
  ).length;
  const safetyViolations = sum(
    [...candidateRuns, ...stableRuns].map(
      (run) => run.verification?.safety_violations ?? 0
    )
  );
  const gates = {
    quality_non_regression:
      state.quality_delta_runs >= contract.minimum_quality_delta,
    completion_non_regression:
      state.completion_delta_runs >= contract.minimum_completion_delta,
    quality_improvement:
      state.quality_delta_runs >= contract.minimum_any_quality_improvement_runs,
    latency_within_limit:
      latencyRatio !== null && latencyRatio <= limits.maximum_median_latency_ratio,
    tokens_within_limit:
      tokenRatio !== null && tokenRatio <= limits.maximum_total_token_ratio,
    setup_failures_within_limit:
      setupFailures <= contract.maximum_setup_failures,
    safety_violations_within_limit:
      safetyViolations <= contract.maximum_safety_violations
  };
  let nextState = state.state;
  let reason = state.reason;
  if (!gates.setup_failures_within_limit) {
    nextState = CLAIM_STATES.INVALID_EVIDENCE;
    reason = "setup_failure_limit_exceeded";
  } else if (
    !gates.quality_non_regression ||
    !gates.completion_non_regression ||
    !gates.safety_violations_within_limit
  ) {
    nextState =
      state.state === CLAIM_STATES.MIXED ? CLAIM_STATES.MIXED : CLAIM_STATES.REGRESSED;
    reason = "non_regression_gate_failed";
  } else if (!gates.quality_improvement) {
    nextState = CLAIM_STATES.NEUTRAL;
    reason = "quality_improvement_threshold_not_met";
  } else if (!gates.latency_within_limit || !gates.tokens_within_limit) {
    nextState = CLAIM_STATES.MIXED;
    reason = "resource_limit_exceeded";
  } else {
    nextState = CLAIM_STATES.IMPROVED;
    reason = "claim_contract_passed";
  }
  return {
    ...state,
    state: nextState,
    reason,
    evidence_complete:
      nextState === CLAIM_STATES.INVALID_EVIDENCE ? false : state.evidence_complete,
    claim_contract_gates: gates,
    median_latency_ratio: latencyRatio,
    total_token_ratio: tokenRatio,
    setup_failures: setupFailures,
    safety_violations: safetyViolations,
    eligible: nextState === CLAIM_STATES.IMPROVED
  };
}

function adaptiveBehaviorChanged(runs, claim) {
  return pairedRuns(runs, claim).some(
    ({ candidate, stable }) =>
      candidate.strategy_receipt?.routing_signature_sha256 !==
      stable.strategy_receipt?.routing_signature_sha256
  );
}

function invalidateUnclassifiedRuns(state, runs, unclassified) {
  const { grounded, auto } = matchedProductRuns(runs);
  const stableByKey = new Map(grounded.map((run) => [pairKey(run), run]));
  const pairs = auto
    .map((candidate) => ({ candidate, stable: stableByKey.get(pairKey(candidate)) }))
    .filter((pair) => pair.stable);
  return {
    ...state,
    state: CLAIM_STATES.INVALID_EVIDENCE,
    reason: "strategy_route_unclassified",
    evidence_complete: false,
    matched_pairs: pairs.length,
    denominator: pairs.length,
    candidate_failed_runs: pairs.filter((pair) => !pair.candidate.completed).length,
    stable_failed_runs: pairs.filter((pair) => !pair.stable.completed).length,
    candidate_timeouts: pairs.filter(
      (pair) => pair.candidate.terminal_status === "timed_out"
    ).length,
    stable_timeouts: pairs.filter(
      (pair) => pair.stable.terminal_status === "timed_out"
    ).length,
    errors: [
      `${unclassified.length} Auto run(s) lacked a classifiable strategy receipt`
    ]
  };
}

export function evaluateCurrentShippingClaims(runs, contract) {
  const claims = {
    adaptive_direct: claimForMode(runs, "adaptive_direct"),
    workflow: claimForMode(runs, "workflow")
  };
  const states = evaluateAgentClaimStates({ runs, claims });
  for (const dimension of ["adaptive_direct", "workflow"]) {
    states[dimension] = applyClaimContract(
      states[dimension],
      runs,
      claims[dimension],
      contract
    );
  }
  if (
    claims.adaptive_direct &&
    states.adaptive_direct.evidence_complete &&
    !adaptiveBehaviorChanged(runs, claims.adaptive_direct)
  ) {
    states.adaptive_direct = {
      ...states.adaptive_direct,
      state: CLAIM_STATES.NEUTRAL,
      reason: "no_observed_behavior_difference",
      eligible: false
    };
  } else if (states.adaptive_direct.state === CLAIM_STATES.NOT_EXERCISED) {
    states.adaptive_direct.reason = "adaptive_direct_not_exercised";
  }
  if (!claims.workflow && states.workflow.state === CLAIM_STATES.NOT_EXERCISED) {
    states.workflow.reason = "workflow_not_exercised";
  }
  const { auto } = matchedProductRuns(runs);
  const unclassified = auto.filter(
    (run) => !["direct", "workflow"].includes(run.strategy_receipt?.execution_mode)
  );
  if (unclassified.length > 0) {
    for (const dimension of ["adaptive_direct", "workflow"]) {
      states[dimension] = invalidateUnclassifiedRuns(
        states[dimension],
        runs,
        unclassified
      );
    }
  }
  states.learned_profile.reason = "exact_stable_parent_not_exercised";
  states.distillation.reason = "exact_stable_parent_not_exercised";
  return states;
}
