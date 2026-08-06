function sum(values) {
  return values.reduce((total, value) => total + value, 0);
}

function percentile(values, fraction) {
  if (values.length === 0) return null;
  const ordered = [...values].sort((left, right) => left - right);
  const index = Math.max(0, Math.ceil(ordered.length * fraction) - 1);
  return ordered[index];
}

function boundedRatio(candidate, baseline) {
  if (baseline === 0) return candidate === 0 ? 1 : null;
  return candidate / baseline;
}

export function pairedDeltas(runs, baseline, treatments) {
  const byKey = new Map(
    runs
      .filter((run) => run.treatment === baseline)
      .map((run) => [`${run.case_id}/r${run.replicate}`, run])
  );
  const result = {};
  for (const treatment of treatments) {
    if (treatment === baseline) continue;
    const pairs = runs
      .filter((run) => run.treatment === treatment)
      .map((run) => [run, byKey.get(`${run.case_id}/r${run.replicate}`)])
      .filter(([, base]) => base);
    result[treatment] = {
      pairs: pairs.length,
      quality_pass_delta: pairs.length
        ? sum(
            pairs.map(
              ([candidate, base]) =>
                Number(candidate.verification.quality_passed) -
                Number(base.verification.quality_passed)
            )
          ) / pairs.length
        : null,
      completion_delta: pairs.length
        ? sum(
            pairs.map(
              ([candidate, base]) => Number(candidate.completed) - Number(base.completed)
            )
          ) / pairs.length
        : null,
      median_latency_delta_ms: pairs.length
        ? percentile(
            pairs.map(
              ([candidate, base]) => candidate.metrics.latency_ms - base.metrics.latency_ms
            ),
            0.5
          )
        : null
    };
  }
  return result;
}

export function promotionDecision(
  suite,
  runs,
  aggregates,
  evidenceComplete,
  setupFailures,
  safetyViolations
) {
  const contract = suite.promotion_v1;
  const baselineRuns = runs.filter((run) => run.treatment === contract.baseline);
  const baseline = aggregates[contract.baseline];
  const candidateGates = {};
  for (const [candidate, limits] of Object.entries(contract.candidates)) {
    const candidateRuns = runs.filter((run) => run.treatment === candidate);
    const qualityDeltaRuns =
      candidateRuns.filter((run) => run.verification.quality_passed).length -
      baselineRuns.filter((run) => run.verification.quality_passed).length;
    const completionDeltaRuns =
      candidateRuns.filter((run) => run.completed).length -
      baselineRuns.filter((run) => run.completed).length;
    const latencyRatio = boundedRatio(
      aggregates[candidate].latency_ms.median,
      baseline.latency_ms.median
    );
    const tokenRatio = boundedRatio(aggregates[candidate].total_tokens, baseline.total_tokens);
    const gates = {
      quality_non_regression: qualityDeltaRuns >= contract.minimum_quality_delta,
      completion_non_regression: completionDeltaRuns >= contract.minimum_completion_delta,
      improves_at_least_one_run:
        Math.max(qualityDeltaRuns, completionDeltaRuns) >=
        contract.minimum_any_improvement_runs,
      latency_within_limit:
        latencyRatio !== null && latencyRatio <= limits.maximum_median_latency_ratio,
      tokens_within_limit:
        tokenRatio !== null && tokenRatio <= limits.maximum_total_token_ratio
    };
    candidateGates[candidate] = {
      quality_delta_runs: qualityDeltaRuns,
      completion_delta_runs: completionDeltaRuns,
      median_latency_ratio: latencyRatio,
      maximum_median_latency_ratio: limits.maximum_median_latency_ratio,
      total_token_ratio: tokenRatio,
      maximum_total_token_ratio: limits.maximum_total_token_ratio,
      gates,
      eligible: Object.values(gates).every(Boolean)
    };
  }
  const sharedGates = {
    receipt_evidence_complete: evidenceComplete,
    setup_failures_within_limit: setupFailures <= contract.maximum_setup_failures,
    safety_violations_within_limit:
      safetyViolations <= contract.maximum_safety_violations,
    all_candidates_preserve_quality_and_completion: Object.values(candidateGates).every(
      (candidate) =>
        candidate.gates.quality_non_regression && candidate.gates.completion_non_regression
    ),
    all_candidates_within_resource_limits: Object.values(candidateGates).every(
      (candidate) =>
        candidate.gates.latency_within_limit && candidate.gates.tokens_within_limit
    ),
    at_least_one_candidate_improves: Object.values(candidateGates).some(
      (candidate) => candidate.gates.improves_at_least_one_run
    )
  };
  return {
    schema: contract.schema,
    baseline: contract.baseline,
    status: Object.values(sharedGates).every(Boolean) ? "GO" : "NO_GO",
    shared_gates: sharedGates,
    candidates: candidateGates,
    minimum_improvement: `${contract.minimum_any_improvement_runs}/${baselineRuns.length}`
  };
}
