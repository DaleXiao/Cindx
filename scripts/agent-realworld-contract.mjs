import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  sanitizeReceiptEvidence,
  toolReceiptMetricFields,
  validateReceiptEvidence,
  validateSuiteReceiptContracts
} from "./agent-realworld-receipts.mjs";
import { evaluateCurrentShippingClaims } from "./agent-realworld-current-claims.mjs";
import { renderCurrentRealworldMarkdown } from "./agent-realworld-current-report.mjs";
import {
  pairedDeltas,
  promotionDecision
} from "./agent-realworld-comparison.mjs";
import {
  EXECUTION_ORDER_PROTOCOL as executionOrderProtocol,
  isCurrentClaimProtocol,
  isOracleReference,
  protocolForSuite
} from "./agent-realworld-protocol.mjs";

const scriptPath = fileURLToPath(import.meta.url);
const setupFailureStages = new Set([
  "project_configuration",
  "workspace_index",
  "memory_seed",
  "recall_session"
]);
const setupFailureCodes = new Set([
  "configuration",
  "index",
  "persistence",
  "data",
  "transient",
  "provider"
]);
const sha256Pattern = /^[0-9a-f]{64}$/;
const budgetFields = [
  "max_duration_ms",
  "model_call_timeout_ms",
  "tool_call_timeout_ms",
  "initial_model_calls",
  "max_model_calls",
  "initial_tool_calls",
  "max_tool_calls",
  "no_progress_timeout_ms",
  "max_total_tokens",
  "max_physical_model_attempts",
  "terminal_token_reserve",
  "terminal_physical_model_attempt_reserve"
];

function requireFact(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function finiteNonNegative(value, label) {
  requireFact(Number.isFinite(value) && value >= 0, `${label} must be non-negative`);
  return value;
}

function rate(numerator, denominator) {
  return denominator === 0 ? null : numerator / denominator;
}

function percentile(values, fraction) {
  if (values.length === 0) return null;
  const ordered = [...values].sort((left, right) => left - right);
  const index = Math.max(0, Math.ceil(ordered.length * fraction) - 1);
  return ordered[index];
}

function sum(values) {
  return values.reduce((total, value) => total + value, 0);
}

function endpointIdentity(value) {
  try {
    const endpoint = new URL(value);
    endpoint.username = "";
    endpoint.password = "";
    endpoint.search = "";
    endpoint.hash = "";
    return endpoint.toString();
  } catch {
    return "invalid-endpoint";
  }
}

function errorKind(value) {
  if (!value) return null;
  const normalized = String(value).toLowerCase();
  if (normalized.includes("permission")) return "permission";
  if (normalized.includes("timeout") || normalized.includes("timed out")) return "timeout";
  if (normalized.includes("cancel")) return "cancelled";
  if (normalized.includes("max") && normalized.includes("turn")) return "turn_limit";
  if (normalized.includes("provider") || normalized.includes("request")) return "provider";
  return "runtime";
}

function runErrorKind(run) {
  if (run.setup_failure?.code) return run.setup_failure.code;
  if (run.terminal_status === "timed_out") return "timeout";
  return errorKind(run.error);
}

function valueCounts(values) {
  const counts = {};
  for (const value of values) {
    if (value == null) continue;
    counts[value] = (counts[value] ?? 0) + 1;
  }
  return counts;
}

function summarizeOutcomes(runs, treatments) {
  const summarize = (selected) => ({
    runs: selected.length,
    completed_runs: selected.filter((run) => run.completed).length,
    non_completed_runs: selected.filter((run) => !run.completed).length,
    terminal_status_counts: valueCounts(selected.map((run) => run.terminal_status)),
    error_kind_counts: valueCounts(selected.map(runErrorKind)),
    setup_failures: selected.filter((run) => run.setup_failure != null).length
  });
  return {
    ...summarize(runs),
    timeouts: runs.filter((run) => run.terminal_status === "timed_out").length,
    permission_waits: runs.filter(
      (run) => run.terminal_status === "waiting_for_permission"
    ).length,
    by_treatment: Object.fromEntries(
      treatments.map((treatment) => [
        treatment,
        summarize(runs.filter((run) => run.treatment === treatment))
      ])
    )
  };
}

function validateSetupFailure(run, key, infrastructureFailed) {
  if (!infrastructureFailed) {
    requireFact(run.setup_failure == null, `${key}: non-setup run must not report setup failure`);
    return;
  }
  const failure = run.setup_failure;
  if (failure == null) return;
  requireFact(typeof failure === "object", `${key}: setup failure is invalid`);
  requireFact(
    setupFailureStages.has(failure.stage),
    `${key}: setup failure stage is invalid`
  );
  requireFact(
    setupFailureCodes.has(failure.code),
    `${key}: setup failure code is invalid`
  );
  requireFact(
    typeof failure.retryable === "boolean",
    `${key}: setup failure retryable flag is missing`
  );
}

function validateSuite(suite) {
  const protocol = protocolForSuite(suite);
  requireFact(typeof suite.id === "string" && suite.id.length > 0, "suite id is missing");
  requireFact(Number.isInteger(suite.version) && suite.version > 0, "suite version is invalid");
  requireFact(
    Number.isInteger(suite.default_replicates) && suite.default_replicates > 0,
    "suite replicate count is invalid"
  );
  requireFact(
    Number.isInteger(suite.per_run_timeout_seconds) &&
      suite.per_run_timeout_seconds >= 60 &&
      suite.per_run_timeout_seconds <= 3600,
    "suite per-run timeout is invalid"
  );
  requireFact(Array.isArray(suite.cases) && suite.cases.length > 0, "suite cases are missing");
  requireFact(
    JSON.stringify(suite.treatments) === JSON.stringify(protocol.treatments),
    "suite treatments do not match the frozen protocol order"
  );
  requireFact(
    suite.execution_order?.protocol === executionOrderProtocol,
    "suite execution-order protocol mismatch"
  );
  requireFact(
    JSON.stringify(suite.execution_order.base_treatments) ===
      JSON.stringify(suite.treatments),
    "suite execution-order treatments must match the frozen treatment order"
  );
  const contract = isCurrentClaimProtocol(suite) ? suite.claim_contract_v2 : suite.promotion_v1;
  requireFact(
    contract?.schema ===
      (isCurrentClaimProtocol(suite)
        ? "cindx.agent-realworld-claims.v2"
        : "cindx.agent-realworld-promotion.v1"),
    "suite claim contract is missing"
  );
  requireFact(
    contract.baseline === protocol.groundedDirect || contract.baseline === "fast",
    "claim baseline does not match the suite protocol"
  );
  if (isCurrentClaimProtocol(suite)) {
    requireFact(contract.oracle_reference === protocol.oracleReference, "oracle reference drifted");
    requireFact(
      JSON.stringify(Object.keys(contract.candidates || {})) === JSON.stringify(["auto", "pro"]),
      "claim candidates must be Auto and Pro"
    );
    requireFact(
      contract.candidates.auto.budget_relation === "iso_budget" &&
        contract.candidates.pro.budget_relation === "descriptive_only",
      "claim budget relations drifted"
    );
  } else {
    requireFact(
      JSON.stringify(Object.keys(contract.candidates || {})) === JSON.stringify(["auto", "pro"]),
      "promotion candidates must be Auto and Pro"
    );
  }
  requireFact(contract.minimum_quality_delta === 0, "claim quality floor must be zero");
  requireFact(contract.minimum_completion_delta === 0, "claim completion floor must be zero");
  const minimumImprovement = isCurrentClaimProtocol(suite)
    ? contract.minimum_any_quality_improvement_runs
    : contract.minimum_any_improvement_runs;
  requireFact(minimumImprovement === 1, "claim must require at least one additional quality pass");
  requireFact(contract.maximum_setup_failures === 0, "claim setup-failure limit must be zero");
  requireFact(contract.maximum_safety_violations === 0, "claim safety limit must be zero");
  for (const [candidate, limits] of Object.entries(contract.candidates)) {
    requireFact(
      Number.isFinite(limits.maximum_median_latency_ratio) &&
        limits.maximum_median_latency_ratio >= 1,
      `${candidate} latency threshold is invalid`
    );
    requireFact(
      Number.isFinite(limits.maximum_total_token_ratio) &&
        limits.maximum_total_token_ratio >= 1,
      `${candidate} token threshold is invalid`
    );
  }
  const ids = new Set();
  for (const testCase of suite.cases) {
    requireFact(typeof testCase.id === "string" && !ids.has(testCase.id), "case ids must be unique");
    ids.add(testCase.id);
    validateSuiteReceiptContracts(testCase);
  }
}

function expectedExecutionEntries(suite) {
  const entries = [];
  let blockIndex = 0;
  for (let replicate = 1; replicate <= suite.default_replicates; replicate += 1) {
    for (const testCase of suite.cases) {
      const rotation = blockIndex % suite.treatments.length;
      for (let position = 0; position < suite.treatments.length; position += 1) {
        entries.push({
          replicate,
          caseId: testCase.id,
          treatment: suite.treatments[(position + rotation) % suite.treatments.length],
          executionIndex: entries.length + 1,
          treatmentPosition: position + 1
        });
      }
      blockIndex += 1;
    }
  }
  return entries;
}

function validateProfileArtifacts(artifacts) {
  requireFact(artifacts && typeof artifacts === "object", "profile artifact bindings are missing");
  requireFact(
    JSON.stringify(Object.keys(artifacts)) === JSON.stringify(["auto", "pro"]),
    "profile artifact bindings must contain Auto and Pro"
  );
  for (const [effort, artifact] of Object.entries(artifacts)) {
    requireFact(
      artifact?.mode === "fresh_seed" || artifact?.mode === "frozen_profile",
      `${effort} profile artifact mode is invalid`
    );
    if (artifact.mode === "fresh_seed") {
      requireFact(
        artifact.file_sha256 === null &&
          artifact.artifact_sha256 === null &&
          artifact.candidate_sha256 === null &&
          artifact.evolution_method === null,
        `${effort} fresh-seed binding must not claim a learned artifact`
      );
    } else {
      requireFact(sha256Pattern.test(artifact.file_sha256), `${effort} profile file hash is invalid`);
      requireFact(
        sha256Pattern.test(artifact.artifact_sha256),
        `${effort} profile artifact hash is invalid`
      );
      requireFact(
        sha256Pattern.test(artifact.candidate_sha256),
        `${effort} profile candidate hash is invalid`
      );
      requireFact(
        typeof artifact.evolution_method === "string" && artifact.evolution_method.length > 0,
        `${effort} profile evolution method is missing`
      );
    }
  }
}

function validateResolvedBudget(run, key) {
  requireFact(run.resolved_budget && typeof run.resolved_budget === "object", `${key}: resolved budget is missing`);
  requireFact(
    JSON.stringify(Object.keys(run.resolved_budget).sort()) === JSON.stringify([...budgetFields].sort()),
    `${key}: resolved budget fields drifted`
  );
  for (const field of budgetFields) {
    requireFact(
      Number.isInteger(run.resolved_budget[field]) && run.resolved_budget[field] >= 0,
      `${key}: resolved budget ${field} is invalid`
    );
  }
  for (const field of [
    "max_duration_ms",
    "model_call_timeout_ms",
    "tool_call_timeout_ms",
    "max_model_calls",
    "max_tool_calls",
    "no_progress_timeout_ms",
    "max_total_tokens",
    "max_physical_model_attempts"
  ]) {
    requireFact(run.resolved_budget[field] > 0, `${key}: resolved budget ${field} must be positive`);
  }
  requireFact(
    run.resolved_budget.max_model_calls >= run.resolved_budget.initial_model_calls,
    `${key}: model-call budget is inconsistent`
  );
  requireFact(
    run.resolved_budget.max_tool_calls >= run.resolved_budget.initial_tool_calls,
    `${key}: tool-call budget is inconsistent`
  );
}

function validateStrategyReceipt(run, key, profileArtifacts, suite) {
  if (isOracleReference(suite, run.treatment)) {
    requireFact(run.strategy_receipt === null, `${key}: oracle reference must not claim a product strategy`);
    return;
  }
  if (run.strategy_receipt === null) {
    requireFact(!run.completed, `${key}: completed product run is missing its strategy receipt`);
    return;
  }
  const receipt = run.strategy_receipt;
  for (const field of [
    "requested_policy",
    "effective_policy",
    "execution_mode",
    "decision_source",
    "profile_source",
    "profile_id"
  ]) {
    requireFact(typeof receipt[field] === "string" && receipt[field].length > 0, `${key}: strategy ${field} is missing`);
  }
  for (const field of ["decision_sha256", "routing_signature_sha256", "profile_sha256"]) {
    requireFact(sha256Pattern.test(receipt[field]), `${key}: strategy ${field} is invalid`);
  }
  requireFact(
    Number.isInteger(receipt.profile_generation) && receipt.profile_generation >= 0,
    `${key}: strategy profile generation is invalid`
  );
  requireFact(Array.isArray(receipt.parent_profile_ids), `${key}: strategy parent profiles are missing`);
  requireFact(
    typeof receipt.workflow_profile_exercised === "boolean",
    `${key}: workflow profile receipt is missing`
  );
  requireFact(
    !run.completed || receipt.execution_mode !== "workflow" || receipt.workflow_profile_exercised,
    `${key}: workflow strategy did not exercise the selected profile`
  );
  const executionConstraint = receipt.execution_constraint ?? "native";
  requireFact(
    executionConstraint === "native" || executionConstraint === "grounded_direct",
    `${key}: execution constraint is invalid`
  );
  if (run.treatment === "grounded_direct") {
    requireFact(
      executionConstraint === "grounded_direct" && receipt.execution_mode === "direct",
      `${key}: grounded direct constraint was not exercised`
    );
  } else {
    requireFact(executionConstraint === "native", `${key}: native treatment claimed a constraint`);
  }
  const artifact = profileArtifacts[run.treatment];
  if (artifact?.mode === "frozen_profile") {
    requireFact(
      receipt.learned_artifact_sha256 === artifact.artifact_sha256,
      `${key}: strategy artifact does not match the frozen execution plan`
    );
    requireFact(
      receipt.learned_method === artifact.evolution_method,
      `${key}: strategy evolution method does not match the frozen execution plan`
    );
    requireFact(receipt.profile_source === "evaluation_frozen_profile", `${key}: frozen strategy source is invalid`);
    requireFact(receipt.profile_sha256 === artifact.candidate_sha256, `${key}: frozen strategy profile hash mismatch`);
    requireFact(typeof receipt.stable_profile_id === "string" && receipt.stable_profile_id.length > 0, `${key}: frozen stable profile is missing`);
    for (const field of ["dataset_sha256", "paired_evidence_sha256"]) {
      requireFact(sha256Pattern.test(receipt[field]), `${key}: frozen strategy ${field} is invalid`);
    }
    requireFact(
      typeof receipt.promotion_gate_protocol === "string" &&
        receipt.promotion_gate_protocol.length > 0,
      `${key}: frozen strategy promotion protocol is missing`
    );
  } else {
    requireFact(
      receipt.learned_artifact_sha256 === null && receipt.learned_method === null,
      `${key}: fresh-seed run must not claim learned-profile evidence`
    );
    requireFact(
      receipt.stable_profile_id === null &&
        receipt.dataset_sha256 === null &&
        receipt.paired_evidence_sha256 === null &&
        receipt.promotion_gate_protocol === null,
      `${key}: fresh-seed run must not retain learned-profile provenance`
    );
  }
}

function validateModelReceipts(run, key) {
  requireFact(Array.isArray(run.model_receipts), `${key}: model receipts are missing`);
  for (const receipt of run.model_receipts) {
    requireFact(typeof receipt.configured_model === "string" && receipt.configured_model.length > 0, `${key}: receipt configured model is missing`);
    requireFact(run.configured_models.includes(receipt.configured_model), `${key}: receipt model was not configured for the run`);
    requireFact(
      receipt.provider_response_model === null ||
        (typeof receipt.provider_response_model === "string" && receipt.provider_response_model.length > 0),
      `${key}: provider response model is invalid`
    );
    requireFact(sha256Pattern.test(receipt.request_payload_sha256), `${key}: request payload hash is invalid`);
    requireFact(sha256Pattern.test(receipt.response_semantic_sha256), `${key}: semantic response hash is invalid`);
    requireFact(
      receipt.provider_system_fingerprint_sha256 === null ||
        sha256Pattern.test(receipt.provider_system_fingerprint_sha256),
      `${key}: provider system fingerprint hash is invalid`
    );
    requireFact(
      ["observed", "provider_id_missing", "identity_conflict"].includes(receipt.receipt_status),
      `${key}: model receipt status is invalid`
    );
    requireFact(!Object.hasOwn(receipt, "provider_response_id"), `${key}: raw provider response id must not be recorded`);
    if (receipt.receipt_status === "observed") {
      requireFact(sha256Pattern.test(receipt.provider_response_id_sha256), `${key}: provider response id hash is missing`);
    } else if (receipt.receipt_status === "provider_id_missing") {
      requireFact(receipt.provider_response_id_sha256 === null, `${key}: missing provider id must be explicit`);
    } else {
      requireFact(
        receipt.provider_response_id_sha256 === null || sha256Pattern.test(receipt.provider_response_id_sha256),
        `${key}: conflicting provider id hash is invalid`
      );
    }
  }
  requireFact(run.model_receipts.length <= run.metrics.model_responses, `${key}: model receipts exceed response events`);
  if (run.completed) {
    requireFact(
      run.model_receipts.length === run.metrics.model_responses,
      `${key}: completed run has incomplete provider response receipts`
    );
  }
}

function validateRun(run, testCase, treatment, replicate, expectedEntry, profileArtifacts, suite) {
  const key = `${testCase.id}/${treatment}/r${replicate}`;
  requireFact(run.case_id === testCase.id, `${key}: case id mismatch`);
  requireFact(run.category === testCase.category, `${key}: category mismatch`);
  requireFact(run.treatment === treatment, `${key}: treatment mismatch`);
  requireFact(run.replicate === replicate, `${key}: replicate mismatch`);
  requireFact(run.execution_index === expectedEntry.executionIndex, `${key}: execution index mismatch`);
  requireFact(run.treatment_position === expectedEntry.treatmentPosition, `${key}: treatment position mismatch`);
  requireFact(typeof run.completed === "boolean", `${key}: completed flag is missing`);
  requireFact(typeof run.terminal_status === "string", `${key}: terminal status is missing`);
  requireFact(
    run.completed === (run.terminal_status === "completed"),
    `${key}: completed flag and terminal status disagree`
  );
  requireFact(
    run.evidence_error === null || typeof run.evidence_error === "string",
    `${key}: evidence error is invalid`
  );
  requireFact(Array.isArray(run.configured_models), `${key}: configured models are missing`);
  requireFact(Array.isArray(run.tools_used), `${key}: tool trace is missing`);
  requireFact(run.tools_used.every((tool) => typeof tool === "string" && tool.length > 0), `${key}: tool trace is invalid`);
  requireFact(/^[0-9a-f]{64}$/.test(run.input_sha256), `${key}: input hash is invalid`);
  requireFact(/^[0-9a-f]{64}$/.test(run.output_sha256), `${key}: output hash is invalid`);
  requireFact(typeof run.output === "string", `${key}: raw output is missing`);
  requireFact(sha256(Buffer.from(run.output)) === run.output_sha256, `${key}: output hash mismatch`);
  requireFact(
    run.product_mechanism_exercised === !isOracleReference(suite, treatment),
    `${key}: product mechanism boundary is incorrect`
  );
  requireFact(run.metrics && typeof run.metrics === "object", `${key}: runtime metrics are missing`);
  for (const metric of [
    "latency_ms",
    "setup_latency_ms",
    "model_calls",
    "model_responses",
    "tool_calls",
    ...Object.values(toolReceiptMetricFields),
    "permission_requests",
    "denied_permissions",
    "recovery_events",
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "context_tokens_used",
    "resident_kib_after",
    "workspace_bytes_after"
  ]) {
    finiteNonNegative(run.metrics[metric], `${key}: ${metric}`);
  }
  requireFact(run.verification && typeof run.verification === "object", `${key}: verification is missing`);
  requireFact(typeof run.verification.quality_passed === "boolean", `${key}: quality result is missing`);
  requireFact(typeof run.verification.answer_passed === "boolean", `${key}: answer result is missing`);
  requireFact(Array.isArray(run.verification.failures), `${key}: verification failures are missing`);
  requireFact(
    run.verification.failures.every((failure) => typeof failure === "string"),
    `${key}: verification failures are invalid`
  );
  const infrastructureFailed = run.terminal_status === "infrastructure_failed";
  validateSetupFailure(run, key, infrastructureFailed);
  if (infrastructureFailed) {
    requireFact(run.completed === false, `${key}: infrastructure failure cannot be completed`);
  }
  requireFact(
    isOracleReference(suite, treatment)
      ? run.verification.external_effect_passed === null
      : infrastructureFailed
        ? run.verification.external_effect_passed === null
        : typeof run.verification.external_effect_passed === "boolean",
    `${key}: external-effect boundary is incorrect`
  );
  finiteNonNegative(run.verification.safety_violations, `${key}: safety violations`);
  requireFact(
    run.verification.quality_passed ===
      (run.verification.answer_passed &&
        (isOracleReference(suite, treatment) || run.verification.external_effect_passed === true)),
    `${key}: quality result is inconsistent with answer and external-effect results`
  );
  validateReceiptEvidence(run, testCase, key);
  validateResolvedBudget(run, key);
  validateStrategyReceipt(run, key, profileArtifacts, suite);
  validateModelReceipts(run, key);
}

function aggregateRuns(runs) {
  const external = runs.filter((run) => run.verification.external_effect_passed !== null);
  return {
    runs: runs.length,
    completion_rate: rate(runs.filter((run) => run.completed).length, runs.length),
    quality_pass_rate: rate(
      runs.filter((run) => run.verification.quality_passed).length,
      runs.length
    ),
    answer_pass_rate: rate(
      runs.filter((run) => run.verification.answer_passed).length,
      runs.length
    ),
    external_effect_pass_rate: rate(
      external.filter((run) => run.verification.external_effect_passed).length,
      external.length
    ),
    safety_violations: sum(runs.map((run) => run.verification.safety_violations)),
    latency_ms: {
      median: percentile(runs.map((run) => run.metrics.latency_ms), 0.5),
      p95: percentile(runs.map((run) => run.metrics.latency_ms), 0.95)
    },
    setup_latency_ms_total: sum(runs.map((run) => run.metrics.setup_latency_ms)),
    total_tokens: sum(runs.map((run) => run.metrics.total_tokens)),
    model_calls: sum(runs.map((run) => run.metrics.model_calls)),
    model_responses: sum(runs.map((run) => run.metrics.model_responses)),
    tool_calls: sum(runs.map((run) => run.metrics.tool_calls)),
    ...Object.fromEntries(
      Object.values(toolReceiptMetricFields).map((field) => [
        field,
        sum(runs.map((run) => run.metrics[field]))
      ])
    ),
    permission_requests: sum(runs.map((run) => run.metrics.permission_requests)),
    denied_permissions: sum(runs.map((run) => run.metrics.denied_permissions)),
    recovery_events: sum(runs.map((run) => run.metrics.recovery_events)),
    peak_resident_kib: Math.max(0, ...runs.map((run) => run.metrics.resident_kib_after)),
    peak_workspace_bytes: Math.max(0, ...runs.map((run) => run.metrics.workspace_bytes_after))
  };
}

function providerEvidenceComplete(run) {
  return (
    run.evidence_error == null &&
    run.model_receipts.length === run.metrics.model_responses &&
    run.model_receipts.every((receipt) => receipt.receipt_status === "observed")
  );
}

function strategyEvidenceComplete(run, suite) {
  return isOracleReference(suite, run.treatment) || run.strategy_receipt !== null;
}

export function validateAndSanitize({ suite, suiteBytes, raw, rawBytes }) {
  validateSuite(suite);
  const protocol = protocolForSuite(suite);
  const currentClaims = isCurrentClaimProtocol(suite);
  requireFact(raw?.schema === protocol.rawSchema, "raw schema mismatch");
  requireFact(raw.suite_id === suite.id, "raw suite id mismatch");
  requireFact(raw.suite_version === suite.version, "raw suite version mismatch");
  requireFact(raw.suite_sha256 === sha256(suiteBytes), "frozen suite SHA-256 mismatch");
  requireFact(/^[0-9a-f]{40}$/.test(raw.git_commit), "evaluated Git commit is invalid");
  requireFact(typeof raw.app_version === "string" && raw.app_version.length > 0, "app version is missing");
  requireFact(raw.provider_id == null || typeof raw.provider_id === "string", "provider id is invalid");
  requireFact(endpointIdentity(raw.provider_endpoint) !== "invalid-endpoint", "provider endpoint is invalid");
  finiteNonNegative(raw.generated_at_ms, "capture time");
  requireFact(raw.requested_replicates === suite.default_replicates, "replicate count drifted from suite");
  requireFact(
    JSON.stringify(raw.selected_cases) === JSON.stringify(suite.cases.map((testCase) => testCase.id)),
    "case selection is incomplete or reordered"
  );
  requireFact(
    JSON.stringify(raw.selected_treatments) === JSON.stringify(suite.treatments),
    "treatment selection is incomplete or reordered"
  );
  requireFact(Array.isArray(raw.runs), "raw runs are missing");
  requireFact(raw.execution_order_protocol === executionOrderProtocol, "raw execution-order protocol mismatch");
  requireFact(sha256Pattern.test(raw.provider_config_sha256), "raw provider configuration binding is invalid");
  validateProfileArtifacts(raw.profile_artifacts);
  const expectedEntries = expectedExecutionEntries(suite);
  const expectedPlanSha256 = sha256(
    JSON.stringify({
      protocol: executionOrderProtocol,
      suite_sha256: raw.suite_sha256,
      git_commit: raw.git_commit,
      replicates: raw.requested_replicates,
      cases: raw.selected_cases,
      treatments: raw.selected_treatments,
      provider_config_sha256: raw.provider_config_sha256,
      profile_artifacts: raw.profile_artifacts,
      entries: expectedEntries
    })
  );
  requireFact(raw.execution_plan_sha256 === expectedPlanSha256, "execution plan SHA-256 mismatch");

  const byKey = new Map();
  for (const [index, run] of raw.runs.entries()) {
    const key = `${run.case_id}/${run.treatment}/r${run.replicate}`;
    requireFact(!byKey.has(key), `duplicate run ${key}`);
    requireFact(run.execution_index === index + 1, `${key}: raw runs are not in execution order`);
    byKey.set(key, run);
  }
  const orderedRuns = [];
  for (let replicate = 1; replicate <= suite.default_replicates; replicate += 1) {
    for (const testCase of suite.cases) {
      for (const treatment of suite.treatments) {
        const key = `${testCase.id}/${treatment}/r${replicate}`;
        const run = byKey.get(key);
        requireFact(run, `missing run ${key}`);
        const expectedEntry = expectedEntries.find(
          (entry) =>
            entry.caseId === testCase.id &&
            entry.treatment === treatment &&
            entry.replicate === replicate
        );
        validateRun(run, testCase, treatment, replicate, expectedEntry, raw.profile_artifacts, suite);
        orderedRuns.push(run);
      }
    }
  }
  requireFact(byKey.size === orderedRuns.length, "raw report contains out-of-contract runs");
  for (const testCase of suite.cases) {
    const hashes = new Set(
      orderedRuns.filter((run) => run.case_id === testCase.id).map((run) => run.input_sha256)
    );
    requireFact(hashes.size === 1, `${testCase.id}: input changed across matched runs`);
  }

  const aggregates = Object.fromEntries(
    suite.treatments.map((treatment) => [
      treatment,
      aggregateRuns(orderedRuns.filter((run) => run.treatment === treatment))
    ])
  );
  const safetyViolations = sum(
    orderedRuns.map((run) => run.verification.safety_violations)
  );
  const incompleteRuns = orderedRuns.filter(
    (run) =>
      run.terminal_status === "infrastructure_failed" ||
      (!isOracleReference(suite, run.treatment) && run.verification.external_effect_passed === null)
  );
  const outcomes = summarizeOutcomes(orderedRuns, suite.treatments);
  const providerEvidenceIncompleteRuns = orderedRuns.filter(
    (run) => !providerEvidenceComplete(run)
  );
  const providerEvidenceCompleteMatrix = providerEvidenceIncompleteRuns.length === 0;
  const strategyEvidenceIncompleteRuns = orderedRuns.filter(
    (run) => !strategyEvidenceComplete(run, suite)
  );
  const receiptEvidenceCompleteMatrix =
    providerEvidenceCompleteMatrix && strategyEvidenceIncompleteRuns.length === 0;
  const promotion = currentClaims
    ? null
    : promotionDecision(
        suite,
        orderedRuns,
        aggregates,
        receiptEvidenceCompleteMatrix && incompleteRuns.length === 0,
        outcomes.setup_failures,
        safetyViolations
      );
  const mechanismClaims = currentClaims
    ? evaluateCurrentShippingClaims(orderedRuns, suite.claim_contract_v2)
    : null;
  const frozenProfiles = Object.entries(raw.profile_artifacts)
    .filter(([, artifact]) => artifact.mode === "frozen_profile")
    .map(([effort]) => effort);
  return {
    schema: protocol.sanitizedSchema,
    suite: {
      id: suite.id,
      version: suite.version,
      description: suite.description,
      sha256: raw.suite_sha256,
      cases: suite.cases.map((testCase) => ({ id: testCase.id, category: testCase.category })),
      treatments: suite.treatments,
      replicates: suite.default_replicates,
      per_run_timeout_seconds: suite.per_run_timeout_seconds,
      execution_order_protocol: executionOrderProtocol,
      ...(currentClaims
        ? { claim_contract_v2: suite.claim_contract_v2 }
        : { promotion_v1: suite.promotion_v1 })
    },
    evidence: {
      raw_sha256: sha256(rawBytes),
      git_commit: raw.git_commit,
      app_version: raw.app_version,
      generated_at_ms: raw.generated_at_ms,
      provider_identity: raw.provider_id?.trim()
        ? { status: "hashed_provider_id", sha256: sha256(raw.provider_id) }
        : { status: "no_provider_id", sha256: null },
      provider_endpoint: endpointIdentity(raw.provider_endpoint),
      configured_models: raw.configured_models,
      complete_matrix: incompleteRuns.length === 0,
      incomplete_runs: incompleteRuns.length,
      product_runs: orderedRuns.filter((run) => run.product_mechanism_exercised).length,
      ...(currentClaims
        ? {
            oracle_reference_runs: orderedRuns.filter(
              (run) => !run.product_mechanism_exercised
            ).length
          }
        : {
            direct_runs: orderedRuns.filter((run) => !run.product_mechanism_exercised).length
          }),
      execution_plan_sha256: raw.execution_plan_sha256,
      profile_artifacts: raw.profile_artifacts,
      provider_evidence_complete: providerEvidenceCompleteMatrix,
      provider_evidence_incomplete_runs: providerEvidenceIncompleteRuns.length,
      strategy_evidence_complete: strategyEvidenceIncompleteRuns.length === 0,
      strategy_evidence_incomplete_runs: strategyEvidenceIncompleteRuns.length
    },
    decision: {
      status:
        incompleteRuns.length > 0 || (currentClaims && !receiptEvidenceCompleteMatrix)
          ? "INVALID_BASELINE"
          : safetyViolations === 0
            ? "VALID_BASELINE"
            : "SAFETY_FAILURE",
      safety_violations: safetyViolations,
      ...(currentClaims
        ? { mechanism_claims: mechanismClaims }
        : {
            uplift_status: promotion.status,
            uplift_reason:
              promotion.status === "GO"
                ? `The preregistered promotion_v1 gates passed; at least one candidate improved by ${promotion.minimum_improvement} without Auto/Pro quality, completion, latency, token, setup, or safety regression.`
                : "At least one preregistered promotion_v1 gate failed; failures remain in the denominator and no uplift is authorized.",
            promotion_v1: promotion,
            learned_profile_status:
              frozenProfiles.length > 0 ? "FROZEN_ARTIFACT_EVALUATED" : "FRESH_SEED_ONLY"
          }),
      claim_boundary:
        incompleteRuns.length > 0
          ? "Infrastructure failures make this matrix invalid for capability promotion or treatment comparison."
          : currentClaims && !receiptEvidenceCompleteMatrix
            ? "Provider or strategy receipt coverage is incomplete, so no mechanism uplift claim is authorized."
            : currentClaims
              ? "Adaptive-direct and workflow conclusions are limited to complete iso-budget GroundedDirect/Auto pairs. Learned-profile and distillation uplift require an actually executed exact stable parent and remain NOT_EXERCISED here. Pro is descriptive because its native budget differs."
          : frozenProfiles.length === 0
            ? "This matrix evaluates fresh-seed product quality only. It provides no evidence of GEPA learning, distillation, learned-profile uplift, or Fugu parity."
            : `This matrix evaluates only the bound frozen ${frozenProfiles.join(" and ")} profile artifact(s); it does not establish general GEPA, distillation, or Fugu parity.`
    },
    aggregates,
    outcomes,
    by_category: Object.fromEntries(
      [...new Set(suite.cases.map((testCase) => testCase.category))].map((category) => [
        category,
        Object.fromEntries(
          suite.treatments.map((treatment) => [
            treatment,
            aggregateRuns(
              orderedRuns.filter(
                (run) => run.category === category && run.treatment === treatment
              )
            )
          ])
        )
      ])
    ),
    paired_against_baseline: pairedDeltas(
      orderedRuns,
      currentClaims ? protocol.groundedDirect : "fast",
      suite.treatments
    ),
    ...(currentClaims
      ? {}
      : { paired_against_fast: pairedDeltas(orderedRuns, "fast", suite.treatments) }),
    runs: orderedRuns.map((run) => {
      const receiptEvidence = sanitizeReceiptEvidence(run);
      return {
        replicate: run.replicate,
        case_id: run.case_id,
        category: run.category,
        treatment: run.treatment,
        execution_index: run.execution_index,
        treatment_position: run.treatment_position,
        product_mechanism_exercised: run.product_mechanism_exercised,
        completed: run.completed,
        terminal_status: run.terminal_status,
        configured_models: run.configured_models,
        tools_used: run.tools_used,
        fixture_receipt: receiptEvidence.fixtureReceipt,
        tool_receipts: receiptEvidence.toolReceipts,
        memory_records_after_seed: run.memory_records_after_seed,
        input_sha256: run.input_sha256,
        output_sha256: run.output_sha256,
        setup_failure: run.setup_failure
          ? {
              stage: run.setup_failure.stage,
              code: run.setup_failure.code,
              retryable: run.setup_failure.retryable
            }
          : null,
        error_kind: runErrorKind(run),
        evidence_error_sha256: run.evidence_error === null ? null : sha256(run.evidence_error),
        resolved_budget: run.resolved_budget,
        strategy_receipt: run.strategy_receipt,
        model_receipts: run.model_receipts,
        metrics: run.metrics,
        verification: receiptEvidence.verification
      };
    })
  };
}

function percent(value) {
  return value === null ? "n/a" : `${(value * 100).toFixed(1)}%`;
}

function signedPercent(value) {
  if (value === null) return "n/a";
  const percentage = value * 100;
  return `${percentage >= 0 ? "+" : ""}${percentage.toFixed(1)} pp`;
}

function signedMilliseconds(value) {
  if (value === null) return "n/a";
  return `${value >= 0 ? "+" : ""}${value} ms`;
}

function statusCount(outcome, status) {
  return outcome.terminal_status_counts[status] ?? 0;
}

export function renderMarkdown(report) {
  if (report.suite.claim_contract_v2) {
    return renderCurrentRealworldMarkdown(report);
  }
  const reportKind = report.decision.status === "VALID_BASELINE" ? "Baseline" : "Evaluation";
  const lines = [
    `# Cindx Agent Real-World ${reportKind} ${report.evidence.app_version}`,
    "",
    "## Evidence",
    "",
    `- Application version: \`${report.evidence.app_version}\``,
    `- Status: **${report.decision.status}**`,
    `- Capture time: \`${new Date(report.evidence.generated_at_ms).toISOString()}\``,
    `- Git commit: \`${report.evidence.git_commit}\``,
    `- Frozen suite: \`${report.suite.id}@${report.suite.version}\` (\`${report.suite.sha256}\`)`,
    `- Suite scope: ${report.suite.description}`,
    `- Raw evidence SHA-256: \`${report.evidence.raw_sha256}\``,
    `- Matrix: ${report.suite.cases.length} cases × ${report.suite.treatments.length} treatments × ${report.suite.replicates} replicates`,
    `- Execution order: ${report.suite.execution_order_protocol} (plan \`${report.evidence.execution_plan_sha256}\`)`,
    `- Missing or structurally unverifiable cells: ${report.evidence.incomplete_runs}`,
    `- Provider-evidence incomplete runs: ${report.evidence.provider_evidence_incomplete_runs}`,
    `- Strategy-evidence incomplete runs: ${report.evidence.strategy_evidence_incomplete_runs}`,
    `- Non-completed runs retained in the denominator: ${report.outcomes.non_completed_runs}`,
    `- Provider identity: ${report.evidence.provider_identity.status}${report.evidence.provider_identity.sha256 ? ` (\`${report.evidence.provider_identity.sha256}\`)` : ""}`,
    `- Provider endpoint: ${report.evidence.provider_endpoint}`,
    "",
    "## Treatment Configuration and Budget",
    "",
    "- Direct is a single-model, no-tools reference with fixture evidence supplied inline; it is not a shipping product treatment.",
    "- Fast, Auto, and Pro execute the shipping AgentKernel, tool, retrieval, memory, and permission paths with their native effort policies.",
    `- Every cell has the same outer process deadline of ${report.suite.per_run_timeout_seconds} seconds. Native effort budgets remain different, so this is a matched shipping-treatment baseline, not an iso-budget comparison.`,
    "",
    "| Model role | Configured model |",
    "| --- | --- |"
  ];
  for (const [role, model] of Object.entries(report.evidence.configured_models)) {
    lines.push(`| ${role} | ${model} |`);
  }
  lines.push(
    "",
    "## Treatment Results",
    "",
    "| Treatment | Complete | Quality | External effect | Safety violations | Median latency | P95 latency | Tokens | Model calls | Tool calls |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
  );
  for (const treatment of report.suite.treatments) {
    const result = report.aggregates[treatment];
    lines.push(
      `| ${treatment} | ${percent(result.completion_rate)} | ${percent(result.quality_pass_rate)} | ${percent(result.external_effect_pass_rate)} | ${result.safety_violations} | ${result.latency_ms.median} ms | ${result.latency_ms.p95} ms | ${result.total_tokens} | ${result.model_calls} | ${result.tool_calls} |`
    );
  }
  lines.push(
    "",
    "## Failure and Permission Outcomes",
    "",
    "| Treatment | Completed | Failed | Timed out | Waiting for permission | Setup failures | Permission requests | Denied |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
  );
  for (const treatment of report.suite.treatments) {
    const outcome = report.outcomes.by_treatment[treatment];
    const aggregate = report.aggregates[treatment];
    lines.push(
      `| ${treatment} | ${outcome.completed_runs} | ${statusCount(outcome, "failed")} | ${statusCount(outcome, "timed_out")} | ${statusCount(outcome, "waiting_for_permission")} | ${outcome.setup_failures} | ${aggregate.permission_requests} | ${aggregate.denied_permissions} |`
    );
  }
  lines.push(
    "",
    `Total: ${report.outcomes.completed_runs} completed, ${report.outcomes.non_completed_runs} non-completed, ${report.outcomes.timeouts} timed out, ${report.outcomes.permission_waits} waiting for permission, and ${report.outcomes.setup_failures} setup failures.`,
    "",
    "## Category Quality",
    "",
    "| Category | Direct | Fast | Auto | Pro |",
    "| --- | ---: | ---: | ---: | ---: |"
  );
  for (const [category, results] of Object.entries(report.by_category)) {
    lines.push(
      `| ${category} | ${percent(results.direct.quality_pass_rate)} | ${percent(results.fast.quality_pass_rate)} | ${percent(results.auto.quality_pass_rate)} | ${percent(results.pro.quality_pass_rate)} |`
    );
  }
  lines.push(
    "",
    "## Paired Against Fast",
    "",
    "| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |",
    "| --- | ---: | ---: | ---: | ---: |"
  );
  for (const treatment of ["auto", "pro"]) {
    const paired = report.paired_against_fast[treatment];
    lines.push(
      `| ${treatment} | ${paired.pairs} | ${signedPercent(paired.quality_pass_delta)} | ${signedPercent(paired.completion_delta)} | ${signedMilliseconds(paired.median_latency_delta_ms)} |`
    );
  }
  lines.push(
    "",
    "## Decisions",
    "",
    `- Baseline validity: **${report.decision.status}**`,
    `- Broad orchestration uplift: **${report.decision.uplift_status.replace("_", "-")}**`,
    `- Uplift rationale: ${report.decision.uplift_reason}`,
    `- Learned-profile evidence: **${report.decision.learned_profile_status.replaceAll("_", "-")}**`,
    `- Required improvement: ${report.decision.promotion_v1.minimum_improvement}`,
    "",
    "| Candidate | Quality delta runs | Completion delta runs | Median latency ratio / max | Total token ratio / max | Eligible |",
    "| --- | ---: | ---: | ---: | ---: | --- |",
    ...["auto", "pro"].map((candidate) => {
      const gate = report.decision.promotion_v1.candidates[candidate];
      return `| ${candidate} | ${gate.quality_delta_runs} | ${gate.completion_delta_runs} | ${gate.median_latency_ratio ?? "n/a"} / ${gate.maximum_median_latency_ratio} | ${gate.total_token_ratio ?? "n/a"} / ${gate.maximum_total_token_ratio} | ${gate.eligible ? "yes" : "no"} |`;
    }),
    "",
    "## Confounds",
    "",
    "- Cells ran serially in a fixed cyclic Latin-square order against one provider and one configured role-model set; the order balances positions but provider and browser conditions can still vary over time.",
    "- The suite contains one fixture per category with three repeats, so category estimates are narrow.",
    "- Direct receives inline fixture evidence and no tools, so it is a reference ceiling rather than an equal product treatment.",
    `- The ${report.suite.per_run_timeout_seconds}-second process deadline is matched, but Fast, Auto, and Pro retain different shipping budgets; timed-out cells can under-report token, call, and resource totals.`,
    report.decision.learned_profile_status === "FRESH_SEED_ONLY"
      ? "- No frozen learned artifact was supplied. These results are fresh-seed quality evidence and cannot be attributed to GEPA, transfer, or self-distillation."
      : "- Learned-profile interpretation is limited to the exact frozen artifact hashes bound into this execution plan.",
    "",
    "## Interpretation Boundary",
    "",
    report.decision.claim_boundary,
    "Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only."
  );
  return `${lines.join("\n")}\n`;
}

function parseArguments(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--help" || flag === "-h") return { help: true };
    const key = {
      "--suite": "suite",
      "--raw": "raw",
      "--sanitized": "sanitized",
      "--markdown": "markdown"
    }[flag];
    requireFact(key, `unknown argument ${flag}`);
    requireFact(argv[index + 1] && !argv[index + 1].startsWith("--"), `${flag} requires a path`);
    options[key] = path.resolve(argv[index + 1]);
    index += 1;
  }
  for (const key of ["suite", "raw", "sanitized", "markdown"]) {
    requireFact(options[key], `--${key} is required`);
  }
  return options;
}

function main(argv = process.argv.slice(2)) {
  const options = parseArguments(argv);
  if (options.help) {
    process.stdout.write(
      "Usage: node scripts/agent-realworld-contract.mjs --suite PATH --raw PATH --sanitized PATH --markdown PATH\n"
    );
    return;
  }
  const suiteBytes = fs.readFileSync(options.suite);
  const rawBytes = fs.readFileSync(options.raw);
  const report = validateAndSanitize({
    suite: JSON.parse(suiteBytes),
    suiteBytes,
    raw: JSON.parse(rawBytes),
    rawBytes
  });
  fs.mkdirSync(path.dirname(options.sanitized), { recursive: true });
  fs.mkdirSync(path.dirname(options.markdown), { recursive: true });
  fs.writeFileSync(options.sanitized, `${JSON.stringify(report, null, 2)}\n`);
  fs.writeFileSync(options.markdown, renderMarkdown(report));
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`Agent real-world contract failed: ${error.message}\n`);
    process.exitCode = 1;
  }
}
