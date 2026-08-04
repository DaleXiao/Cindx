import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
  requireFact(suite?.schema === "cindx.agent-realworld-suite.v1", "suite schema mismatch");
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
    JSON.stringify(suite.treatments) === JSON.stringify(["direct", "fast", "auto", "pro"]),
    "suite treatments must be Direct/Fast/Auto/Pro in frozen order"
  );
  const ids = new Set();
  for (const testCase of suite.cases) {
    requireFact(typeof testCase.id === "string" && !ids.has(testCase.id), "case ids must be unique");
    ids.add(testCase.id);
  }
}

function validateRun(run, testCase, treatment, replicate) {
  const key = `${testCase.id}/${treatment}/r${replicate}`;
  requireFact(run.case_id === testCase.id, `${key}: case id mismatch`);
  requireFact(run.category === testCase.category, `${key}: category mismatch`);
  requireFact(run.treatment === treatment, `${key}: treatment mismatch`);
  requireFact(run.replicate === replicate, `${key}: replicate mismatch`);
  requireFact(typeof run.completed === "boolean", `${key}: completed flag is missing`);
  requireFact(typeof run.terminal_status === "string", `${key}: terminal status is missing`);
  requireFact(Array.isArray(run.configured_models), `${key}: configured models are missing`);
  requireFact(Array.isArray(run.tools_used), `${key}: tool trace is missing`);
  requireFact(/^[0-9a-f]{64}$/.test(run.input_sha256), `${key}: input hash is invalid`);
  requireFact(/^[0-9a-f]{64}$/.test(run.output_sha256), `${key}: output hash is invalid`);
  requireFact(typeof run.output === "string", `${key}: raw output is missing`);
  requireFact(sha256(Buffer.from(run.output)) === run.output_sha256, `${key}: output hash mismatch`);
  requireFact(
    run.product_mechanism_exercised === (treatment !== "direct"),
    `${key}: product mechanism boundary is incorrect`
  );
  requireFact(run.metrics && typeof run.metrics === "object", `${key}: runtime metrics are missing`);
  for (const metric of [
    "latency_ms",
    "setup_latency_ms",
    "model_calls",
    "tool_calls",
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
  const infrastructureFailed = run.terminal_status === "infrastructure_failed";
  validateSetupFailure(run, key, infrastructureFailed);
  if (infrastructureFailed) {
    requireFact(run.completed === false, `${key}: infrastructure failure cannot be completed`);
  }
  requireFact(
    treatment === "direct"
      ? run.verification.external_effect_passed === null
      : infrastructureFailed
        ? run.verification.external_effect_passed === null
        : typeof run.verification.external_effect_passed === "boolean",
    `${key}: external-effect boundary is incorrect`
  );
  finiteNonNegative(run.verification.safety_violations, `${key}: safety violations`);
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
    tool_calls: sum(runs.map((run) => run.metrics.tool_calls)),
    permission_requests: sum(runs.map((run) => run.metrics.permission_requests)),
    denied_permissions: sum(runs.map((run) => run.metrics.denied_permissions)),
    recovery_events: sum(runs.map((run) => run.metrics.recovery_events)),
    peak_resident_kib: Math.max(0, ...runs.map((run) => run.metrics.resident_kib_after)),
    peak_workspace_bytes: Math.max(0, ...runs.map((run) => run.metrics.workspace_bytes_after))
  };
}

function pairedDeltas(runs, baseline) {
  const byKey = new Map(
    runs
      .filter((run) => run.treatment === baseline)
      .map((run) => [`${run.case_id}/r${run.replicate}`, run])
  );
  const result = {};
  for (const treatment of ["direct", "fast", "auto", "pro"]) {
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
        ? sum(pairs.map(([candidate, base]) => Number(candidate.completed) - Number(base.completed))) /
          pairs.length
        : null,
      median_latency_delta_ms: pairs.length
        ? percentile(pairs.map(([candidate, base]) => candidate.metrics.latency_ms - base.metrics.latency_ms), 0.5)
        : null
    };
  }
  return result;
}

export function validateAndSanitize({ suite, suiteBytes, raw, rawBytes }) {
  validateSuite(suite);
  requireFact(raw?.schema === "cindx.agent-realworld-raw.v1", "raw schema mismatch");
  requireFact(raw.suite_id === suite.id, "raw suite id mismatch");
  requireFact(raw.suite_version === suite.version, "raw suite version mismatch");
  requireFact(raw.suite_sha256 === sha256(suiteBytes), "frozen suite SHA-256 mismatch");
  requireFact(/^[0-9a-f]{40}$/.test(raw.git_commit), "evaluated Git commit is invalid");
  requireFact(typeof raw.app_version === "string" && raw.app_version.length > 0, "app version is missing");
  requireFact(typeof raw.provider_id === "string" && raw.provider_id.length > 0, "provider id is missing");
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

  const byKey = new Map();
  for (const run of raw.runs) {
    const key = `${run.case_id}/${run.treatment}/r${run.replicate}`;
    requireFact(!byKey.has(key), `duplicate run ${key}`);
    byKey.set(key, run);
  }
  const orderedRuns = [];
  for (let replicate = 1; replicate <= suite.default_replicates; replicate += 1) {
    for (const testCase of suite.cases) {
      for (const treatment of suite.treatments) {
        const key = `${testCase.id}/${treatment}/r${replicate}`;
        const run = byKey.get(key);
        requireFact(run, `missing run ${key}`);
        validateRun(run, testCase, treatment, replicate);
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
      (run.treatment !== "direct" && run.verification.external_effect_passed === null)
  );
  const outcomes = summarizeOutcomes(orderedRuns, suite.treatments);
  return {
    schema: "cindx.agent-realworld-sanitized.v1",
    suite: {
      id: suite.id,
      version: suite.version,
      description: suite.description,
      sha256: raw.suite_sha256,
      cases: suite.cases.map((testCase) => ({ id: testCase.id, category: testCase.category })),
      treatments: suite.treatments,
      replicates: suite.default_replicates,
      per_run_timeout_seconds: suite.per_run_timeout_seconds
    },
    evidence: {
      raw_sha256: sha256(rawBytes),
      git_commit: raw.git_commit,
      app_version: raw.app_version,
      generated_at_ms: raw.generated_at_ms,
      provider_id: raw.provider_id,
      provider_endpoint: endpointIdentity(raw.provider_endpoint),
      configured_models: raw.configured_models,
      complete_matrix: incompleteRuns.length === 0,
      incomplete_runs: incompleteRuns.length,
      product_runs: orderedRuns.filter((run) => run.product_mechanism_exercised).length,
      direct_runs: orderedRuns.filter((run) => !run.product_mechanism_exercised).length
    },
    decision: {
      status:
        incompleteRuns.length > 0
          ? "INVALID_BASELINE"
          : safetyViolations === 0
            ? "VALID_BASELINE"
            : "SAFETY_FAILURE",
      safety_violations: safetyViolations,
      uplift_status: "NO_GO",
      uplift_reason:
        "This descriptive V2 baseline has no preregistered promotion threshold and cannot authorize a broad orchestration-uplift claim.",
      claim_boundary:
        incompleteRuns.length > 0
          ? "Infrastructure failures make this matrix invalid for capability promotion or treatment comparison."
          : "This is a matched product baseline, not evidence of Fugu Ultra parity or causal intelligence uplift."
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
    paired_against_fast: pairedDeltas(orderedRuns, "fast"),
    runs: orderedRuns.map((run) => ({
      replicate: run.replicate,
      case_id: run.case_id,
      category: run.category,
      treatment: run.treatment,
      product_mechanism_exercised: run.product_mechanism_exercised,
      completed: run.completed,
      terminal_status: run.terminal_status,
      configured_models: run.configured_models,
      tools_used: run.tools_used,
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
      metrics: run.metrics,
      verification: run.verification
    }))
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
    `- Missing or structurally unverifiable cells: ${report.evidence.incomplete_runs}`,
    `- Non-completed runs retained in the denominator: ${report.outcomes.non_completed_runs}`,
    `- Provider: ${report.evidence.provider_id} (${report.evidence.provider_endpoint})`,
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
    "",
    "## Confounds",
    "",
    "- Cells ran serially in one frozen order against one provider and one configured role-model set; provider and browser conditions can vary over time.",
    "- The suite contains one fixture per category with three repeats, so category estimates are narrow.",
    "- Direct receives inline fixture evidence and no tools, so it is a reference ceiling rather than an equal product treatment.",
    `- The ${report.suite.per_run_timeout_seconds}-second process deadline is matched, but Fast, Auto, and Pro retain different shipping budgets; timed-out cells can under-report token, call, and resource totals.`,
    "- Raw v1 evidence does not capture learned profile or GEPA identities, so results cannot be attributed to GEPA, transfer, or self-distillation.",
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
