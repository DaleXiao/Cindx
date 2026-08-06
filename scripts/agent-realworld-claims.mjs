import crypto from "node:crypto";

export const CLAIM_DIMENSIONS = Object.freeze([
  "adaptive_direct",
  "workflow",
  "learned_profile",
  "distillation"
]);

export const CLAIM_STATES = Object.freeze({
  NOT_EXERCISED: "NOT_EXERCISED",
  INVALID_EVIDENCE: "INVALID_EVIDENCE",
  NEUTRAL: "NEUTRAL",
  IMPROVED: "IMPROVED",
  REGRESSED: "REGRESSED",
  MIXED: "MIXED"
});

const exactParentDimensions = new Set(["learned_profile", "distillation"]);
const sha256Pattern = /^[0-9a-f]{64}$/;

function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableValue(value[key])])
    );
  }
  return value;
}

function stableJson(value) {
  return JSON.stringify(stableValue(value));
}

function sha256Value(value) {
  return crypto.createHash("sha256").update(stableJson(value)).digest("hex");
}

function nonEmpty(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function lowerSha256(value) {
  return typeof value === "string" && sha256Pattern.test(value);
}

function pairKey(run) {
  if (!nonEmpty(run?.case_id) || !Number.isInteger(run?.replicate) || run.replicate < 1) {
    return null;
  }
  return `${run.case_id}/r${run.replicate}`;
}

function claimReceipt(run, dimension) {
  const explicit = run?.claim_receipts?.[dimension];
  if (explicit === true) return { exercised: true };
  if (explicit && typeof explicit === "object") return explicit;

  const strategy = run?.strategy_receipt;
  if (!strategy) return null;
  if (dimension === "adaptive_direct") {
    return {
      exercised:
        run.product_mechanism_exercised === true && strategy.execution_mode === "direct"
    };
  }
  if (dimension === "workflow") {
    return {
      exercised:
        strategy.execution_mode === "workflow" && strategy.workflow_profile_exercised === true
    };
  }
  if (dimension === "learned_profile") {
    return {
      exercised:
        strategy.profile_source === "evaluation_frozen_profile" &&
        lowerSha256(strategy.learned_artifact_sha256)
    };
  }
  return {
    exercised:
      strategy.profile_source === "evaluation_frozen_profile" &&
      strategy.learned_method === "pro_to_auto_distillation"
  };
}

function profileBinding(side) {
  const profile = side?.profile;
  if (!profile || !nonEmpty(profile.id) || !lowerSha256(profile.sha256)) return null;
  return profile;
}

function strategyProfile(run) {
  const receipt = run?.strategy_receipt;
  if (!receipt || !nonEmpty(receipt.profile_id) || !lowerSha256(receipt.profile_sha256)) {
    return null;
  }
  return receipt;
}

function modelBinding(run) {
  if (lowerSha256(run?.model_binding_sha256)) {
    return { digest: run.model_binding_sha256, value: null };
  }
  const models = run?.configured_models;
  if (Array.isArray(models) && models.every(nonEmpty)) {
    const normalized = [...models].sort();
    return { digest: sha256Value(normalized), value: normalized };
  }
  if (
    models &&
    typeof models === "object" &&
    Object.values(models).every((model) => typeof model === "string")
  ) {
    return { digest: sha256Value(models), value: stableValue(models) };
  }
  return null;
}

function budgetBinding(run) {
  if (lowerSha256(run?.budget_sha256)) {
    return { digest: run.budget_sha256, value: null };
  }
  const budget = run?.resolved_budget;
  if (!budget || typeof budget !== "object" || Array.isArray(budget)) return null;
  return { digest: sha256Value(budget), value: stableValue(budget) };
}

function outcome(run) {
  const quality = run?.verification?.quality_passed;
  if (
    typeof quality !== "boolean" ||
    typeof run?.completed !== "boolean" ||
    !nonEmpty(run?.terminal_status)
  ) {
    return null;
  }
  const latency = run?.metrics?.latency_ms;
  const tokens = run?.metrics?.total_tokens;
  if (!Number.isFinite(latency) || latency < 0 || !Number.isFinite(tokens) || tokens < 0) {
    return null;
  }
  return {
    quality,
    completed: run.completed,
    terminal_status: run.terminal_status,
    latency_ms: latency,
    total_tokens: tokens
  };
}

function providerEvidenceErrors(run, label) {
  const errors = [];
  if (run?.evidence_error != null) {
    errors.push(`${label}: provider evidence reports an error`);
  }
  if (
    !Array.isArray(run?.model_receipts) ||
    !Number.isInteger(run?.metrics?.model_responses) ||
    run.model_receipts.length !== run.metrics.model_responses ||
    !run.model_receipts.every((receipt) => receipt?.receipt_status === "observed")
  ) {
    errors.push(`${label}: provider response receipt coverage is incomplete`);
  }
  return errors;
}

function baseResult(dimension, state, reason) {
  return {
    dimension,
    state,
    reason,
    evidence_complete: false,
    matched_pairs: 0,
    denominator: 0,
    quality_delta_runs: 0,
    completion_delta_runs: 0,
    candidate_failed_runs: 0,
    stable_failed_runs: 0,
    candidate_timeouts: 0,
    stable_timeouts: 0,
    candidate_latency_ms_total: 0,
    stable_latency_ms_total: 0,
    candidate_tokens_total: 0,
    stable_tokens_total: 0,
    errors: []
  };
}

function notExercised(dimension, reason, matchedPairs = 0) {
  return {
    ...baseResult(dimension, CLAIM_STATES.NOT_EXERCISED, reason),
    matched_pairs: matchedPairs,
    denominator: matchedPairs
  };
}

function invalidEvidence(dimension, errors, matchedPairs = 0) {
  return {
    ...baseResult(dimension, CLAIM_STATES.INVALID_EVIDENCE, "evidence_failed_closed"),
    matched_pairs: matchedPairs,
    denominator: matchedPairs,
    errors
  };
}

function indexedRuns(runs, treatment, dimension) {
  const selected = runs.filter((run) => run?.treatment === treatment);
  const indexed = new Map();
  const errors = [];
  for (const run of selected) {
    const key = pairKey(run);
    if (key === null) {
      errors.push(`${dimension}/${treatment}: invalid case or replicate identity`);
      continue;
    }
    if (indexed.has(key)) {
      errors.push(`${dimension}/${treatment}/${key}: duplicate matched cell`);
      continue;
    }
    indexed.set(key, run);
  }
  return { indexed, errors };
}

function expectedProfileErrors(run, expected, label) {
  const receipt = strategyProfile(run);
  if (receipt === null) return [`${label}: profile receipt is missing`];
  const errors = [];
  if (receipt.profile_id !== expected.id) errors.push(`${label}: profile id mismatch`);
  if (receipt.profile_sha256 !== expected.sha256) errors.push(`${label}: profile hash mismatch`);
  if (expected.source !== undefined && receipt.profile_source !== expected.source) {
    errors.push(`${label}: profile source mismatch`);
  }
  if (expected.pair_sha256 !== undefined) {
    if (!lowerSha256(expected.pair_sha256)) {
      errors.push(`${label}: expected profile pair hash is invalid`);
    } else if (receipt.profile_pair_sha256 !== expected.pair_sha256) {
      errors.push(`${label}: profile pair hash mismatch`);
    }
  }
  if (expected.learned_method !== undefined && receipt.learned_method !== expected.learned_method) {
    errors.push(`${label}: learned method mismatch`);
  }
  if (expected.teacher_profile_id !== undefined) {
    if (receipt.teacher_profile_id !== expected.teacher_profile_id) {
      errors.push(`${label}: teacher profile id mismatch`);
    }
  }
  if (expected.teacher_profile_sha256 !== undefined) {
    if (receipt.teacher_profile_sha256 !== expected.teacher_profile_sha256) {
      errors.push(`${label}: teacher profile hash mismatch`);
    }
  }
  return errors;
}

function exactParentPresence(candidate, stableProfile) {
  const expected = candidate?.profile;
  const receipt = candidate?.run?.strategy_receipt;
  if (
    !nonEmpty(expected?.exact_parent_profile_id) ||
    !lowerSha256(expected?.exact_parent_profile_sha256) ||
    !nonEmpty(stableProfile?.id) ||
    !lowerSha256(stableProfile?.sha256) ||
    !nonEmpty(receipt?.stable_profile_id) ||
    !lowerSha256(receipt?.stable_profile_sha256)
  ) {
    return false;
  }
  return true;
}

function exactParentErrors(candidate, stableProfile, label) {
  const expected = candidate.profile;
  const receipt = candidate.run.strategy_receipt;
  const errors = [];
  if (expected.exact_parent_profile_id !== stableProfile.id) {
    errors.push(`${label}: configured parent id does not name the stable profile`);
  }
  if (expected.exact_parent_profile_sha256 !== stableProfile.sha256) {
    errors.push(`${label}: configured parent hash does not name the stable profile`);
  }
  if (receipt.stable_profile_id !== stableProfile.id) {
    errors.push(`${label}: exercised parent id does not name the stable profile`);
  }
  if (receipt.stable_profile_sha256 !== stableProfile.sha256) {
    errors.push(`${label}: exercised parent hash does not name the stable profile`);
  }
  return errors;
}

function resultState(qualityDelta, completionDelta) {
  if (qualityDelta === 0 && completionDelta === 0) return CLAIM_STATES.NEUTRAL;
  if (qualityDelta > 0 && completionDelta >= 0) return CLAIM_STATES.IMPROVED;
  if (qualityDelta === 0 && completionDelta > 0) return CLAIM_STATES.IMPROVED;
  if (qualityDelta < 0 && completionDelta <= 0) return CLAIM_STATES.REGRESSED;
  if (qualityDelta === 0 && completionDelta < 0) return CLAIM_STATES.REGRESSED;
  return CLAIM_STATES.MIXED;
}

export function evaluateClaimState({ dimension, runs = [], claim } = {}) {
  if (!CLAIM_DIMENSIONS.includes(dimension)) {
    throw new Error(`unsupported claim dimension ${dimension}`);
  }
  if (!claim || typeof claim !== "object") {
    return notExercised(dimension, "claim_not_configured");
  }
  const candidateTreatment = claim.candidate?.treatment;
  const stableTreatment = claim.stable?.treatment;
  if (
    !nonEmpty(candidateTreatment) ||
    !nonEmpty(stableTreatment) ||
    candidateTreatment === stableTreatment
  ) {
    return notExercised(dimension, "matched_treatments_not_configured");
  }

  const candidateIndex = indexedRuns(runs, candidateTreatment, dimension);
  const stableIndex = indexedRuns(runs, stableTreatment, dimension);
  const structuralErrors = [...candidateIndex.errors, ...stableIndex.errors];
  const allCandidateKeys = [...candidateIndex.indexed.keys()].sort();
  const allStableKeys = [...stableIndex.indexed.keys()].sort();
  let candidateKeys = allCandidateKeys;
  if (claim.pair_keys !== undefined) {
    if (
      !Array.isArray(claim.pair_keys) ||
      claim.pair_keys.length === 0 ||
      claim.pair_keys.some((key) => !nonEmpty(key)) ||
      new Set(claim.pair_keys).size !== claim.pair_keys.length
    ) {
      structuralErrors.push(`${dimension}: selected matched cells are invalid`);
      candidateKeys = [];
    } else {
      candidateKeys = [...claim.pair_keys].sort();
      for (const key of candidateKeys) {
        if (!candidateIndex.indexed.has(key) || !stableIndex.indexed.has(key)) {
          structuralErrors.push(`${dimension}/${key}: selected matched cell is missing`);
        }
      }
    }
  } else if (stableJson(allCandidateKeys) !== stableJson(allStableKeys)) {
    structuralErrors.push(`${dimension}: candidate and stable matched cells differ`);
  }
  if (candidateKeys.length === 0) {
    return notExercised(dimension, "no_matched_observations");
  }
  if (
    claim.expected_pairs !== undefined &&
    (!Number.isInteger(claim.expected_pairs) || claim.expected_pairs < 1 ||
      candidateKeys.length !== claim.expected_pairs)
  ) {
    structuralErrors.push(`${dimension}: matched pair count differs from the frozen plan`);
  }
  if (structuralErrors.length > 0) {
    return invalidEvidence(dimension, structuralErrors, candidateKeys.length);
  }

  const pairs = candidateKeys.map((key) => ({
    key,
    candidate: candidateIndex.indexed.get(key),
    stable: stableIndex.indexed.get(key)
  }));
  const everyCandidateExercised = pairs.every(
    ({ candidate }) => claimReceipt(candidate, dimension)?.exercised === true
  );
  if (!everyCandidateExercised) {
    return notExercised(dimension, "candidate_behavior_not_exercised", pairs.length);
  }

  const candidateProfile = profileBinding(claim.candidate);
  const stableProfile = profileBinding(claim.stable);
  if (candidateProfile === null || stableProfile === null) {
    return notExercised(dimension, "exact_profile_bindings_missing", pairs.length);
  }
  if (exactParentDimensions.has(dimension)) {
    const parentPresent = pairs.every(({ candidate }) =>
      exactParentPresence(
        { run: candidate, profile: candidateProfile },
        stableProfile
      )
    );
    if (!parentPresent) {
      return notExercised(dimension, "exact_stable_parent_not_exercised", pairs.length);
    }
  }

  const errors = [];
  const observations = [];
  for (const { key, candidate, stable } of pairs) {
    const label = `${dimension}/${key}`;
    if (!lowerSha256(candidate.input_sha256) || candidate.input_sha256 !== stable.input_sha256) {
      errors.push(`${label}: matched input hash mismatch`);
    }
    if (claim.require_provider_evidence === true) {
      errors.push(
        ...providerEvidenceErrors(candidate, `${label}/candidate`),
        ...providerEvidenceErrors(stable, `${label}/stable`)
      );
    }

    const candidateBudget = budgetBinding(candidate);
    const stableBudget = budgetBinding(stable);
    if (candidateBudget === null || stableBudget === null) {
      errors.push(`${label}: budget receipt is missing`);
    } else if (candidateBudget.digest !== stableBudget.digest) {
      errors.push(`${label}: budget receipt mismatch`);
    }
    if (lowerSha256(claim.budget_sha256)) {
      if (candidateBudget?.digest !== claim.budget_sha256 || stableBudget?.digest !== claim.budget_sha256) {
        errors.push(`${label}: budget receipt differs from the frozen plan`);
      }
    }

    const candidateModels = modelBinding(candidate);
    const stableModels = modelBinding(stable);
    if (candidateModels === null || stableModels === null) {
      errors.push(`${label}: model binding receipt is missing`);
    } else if (candidateModels.digest !== stableModels.digest) {
      errors.push(`${label}: model binding receipt mismatch`);
    }
    if (lowerSha256(claim.model_binding_sha256)) {
      if (
        candidateModels?.digest !== claim.model_binding_sha256 ||
        stableModels?.digest !== claim.model_binding_sha256
      ) {
        errors.push(`${label}: model binding differs from the frozen plan`);
      }
    }

    errors.push(
      ...expectedProfileErrors(candidate, candidateProfile, `${label}/candidate`),
      ...expectedProfileErrors(stable, stableProfile, `${label}/stable`)
    );
    if (exactParentDimensions.has(dimension)) {
      errors.push(
        ...exactParentErrors(
          { run: candidate, profile: candidateProfile },
          stableProfile,
          `${label}/candidate`
        )
      );
    }

    const candidateOutcome = outcome(candidate);
    const stableOutcome = outcome(stable);
    if (candidateOutcome === null || stableOutcome === null) {
      errors.push(`${label}: outcome receipt is incomplete`);
    } else {
      observations.push({ candidate: candidateOutcome, stable: stableOutcome });
    }
  }
  if (errors.length > 0) {
    return invalidEvidence(dimension, errors, pairs.length);
  }

  const totals = observations.reduce(
    (summary, pair) => {
      summary.quality_delta_runs += Number(pair.candidate.quality) - Number(pair.stable.quality);
      summary.completion_delta_runs +=
        Number(pair.candidate.completed) - Number(pair.stable.completed);
      summary.candidate_failed_runs += Number(!pair.candidate.completed);
      summary.stable_failed_runs += Number(!pair.stable.completed);
      summary.candidate_timeouts += Number(pair.candidate.terminal_status === "timed_out");
      summary.stable_timeouts += Number(pair.stable.terminal_status === "timed_out");
      summary.candidate_latency_ms_total += pair.candidate.latency_ms;
      summary.stable_latency_ms_total += pair.stable.latency_ms;
      summary.candidate_tokens_total += pair.candidate.total_tokens;
      summary.stable_tokens_total += pair.stable.total_tokens;
      return summary;
    },
    {
      quality_delta_runs: 0,
      completion_delta_runs: 0,
      candidate_failed_runs: 0,
      stable_failed_runs: 0,
      candidate_timeouts: 0,
      stable_timeouts: 0,
      candidate_latency_ms_total: 0,
      stable_latency_ms_total: 0,
      candidate_tokens_total: 0,
      stable_tokens_total: 0
    }
  );
  const state = resultState(totals.quality_delta_runs, totals.completion_delta_runs);
  return {
    ...baseResult(dimension, state, "complete_matched_evidence"),
    evidence_complete: true,
    matched_pairs: observations.length,
    denominator: observations.length,
    ...totals
  };
}

export function evaluateAgentClaimStates({ runs = [], claims = {} } = {}) {
  return Object.fromEntries(
    CLAIM_DIMENSIONS.map((dimension) => [
      dimension,
      evaluateClaimState({ dimension, runs, claim: claims[dimension] })
    ])
  );
}
