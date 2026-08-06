import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const sha256Pattern = /^[0-9a-f]{64}$/;
const treatments = ["memory_on", "memory_off"];

function requireFact(condition, message) {
  if (!condition) throw new Error(message);
}

function digest(domain, value) {
  return crypto.createHash("sha256").update(`${domain}\0${value}`).digest("hex");
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function digestJson(domain, value) {
  return digest(domain, JSON.stringify(value));
}

function counts(values) {
  const result = {};
  for (const value of values) result[value] = (result[value] ?? 0) + 1;
  return result;
}

function parseArguments(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const key = { "--suite": "suite", "--raw": "raw", "--sanitized": "sanitized", "--markdown": "markdown" }[flag];
    requireFact(key, `unknown argument ${flag}`);
    requireFact(argv[index + 1], `${flag} requires a value`);
    options[key] = argv[index + 1];
  }
  for (const key of ["suite", "raw", "sanitized", "markdown"]) {
    requireFact(options[key], `--${key} is required`);
  }
  return options;
}

function validateSuite(suite) {
  requireFact(suite?.schema === "cindx.agent-memory-effect-suite.v1", "suite schema mismatch");
  requireFact(suite.default_replicates === 3, "suite must freeze three replicates");
  requireFact(JSON.stringify(suite.treatments) === JSON.stringify(treatments), "suite treatment order mismatch");
  requireFact(suite.execution_order?.protocol === "cyclic_latin_square_v1", "suite execution order mismatch");
  requireFact(JSON.stringify(suite.execution_order.base_treatments) === JSON.stringify(treatments), "suite base treatments mismatch");
  const contract = suite.memory_effect_contract_v1;
  requireFact(
    contract?.schema === "cindx.agent-memory-effect-contract.v1" &&
      contract.on === "memory_on" && contract.off === "memory_off" &&
      contract.required_cases === 2 && contract.irrelevant_controls === 1 &&
      contract.replicates === 3 && contract.cells === 18,
    "suite memory-effect contract mismatch"
  );
  requireFact(suite.cases?.length === 3, "suite must contain three cases");
  const roles = suite.cases.map((testCase) => testCase.memory_effect?.role);
  requireFact(roles.filter((role) => role === "required").length === 2, "suite must contain two required cases");
  requireFact(roles.filter((role) => role === "irrelevant_control").length === 1, "suite must contain one irrelevant control");
  for (const testCase of suite.cases) {
    requireFact(testCase.seed_memory_prompt?.trim(), `${testCase.id}: seed memory is missing`);
  }
  const control = suite.cases.find((testCase) => testCase.memory_effect.role === "irrelevant_control");
  requireFact(control.verification?.output_not_contains?.length > 0, "irrelevant control decoy contract is missing");
  const readOnlyTools = new Set(["file.list", "file.read", "file.read_many", "file.search", "tool.inspect", "tool.search"]);
  for (const testCase of suite.cases) {
    requireFact(testCase.permission_policy === "deny_mutations", `${testCase.id}: mutations are not denied`);
    requireFact(testCase.verification?.allowed_tools?.length > 0, `${testCase.id}: tool allowlist is missing`);
    requireFact(testCase.verification.allowed_tools.every((tool) => readOnlyTools.has(tool)), `${testCase.id}: tool allowlist is not read-only`);
  }
}

function expectedCells(suite) {
  const cells = [];
  let block = 0;
  for (let replicate = 1; replicate <= 3; replicate += 1) {
    for (const testCase of suite.cases) {
      const rotation = block % treatments.length;
      for (let position = 0; position < treatments.length; position += 1) {
        cells.push({
          key: `${testCase.id}/${treatments[(position + rotation) % treatments.length]}/r${replicate}`,
          caseId: testCase.id,
          role: testCase.memory_effect.role,
          treatment: treatments[(position + rotation) % treatments.length],
          replicate,
          executionIndex: cells.length + 1,
          treatmentPosition: position + 1
        });
      }
      block += 1;
    }
  }
  return cells;
}

function receiptValid(run) {
  const receipt = run.memory_evaluation_receipt;
  if (!receipt || !sha256Pattern.test(run.memory_seed_sha256 || "")) return false;
  if (!["none", "relevant", "comprehensive"].includes(receipt.routed_memory_policy)) return false;
  if (![receipt.routed_query_sha256, receipt.selected_memory_ids_sha256, receipt.non_memory_decision_sha256].every((value) => sha256Pattern.test(value || ""))) return false;
  if (!Number.isInteger(receipt.recall_count) || receipt.recall_count < 0 || !Number.isInteger(receipt.selected_count) || receipt.selected_count < 0) return false;
  if (run.treatment === "memory_off") {
    return receipt.constraint === "memory_off" && receipt.effective_memory_policy === "none" && receipt.recall_count === 0 && receipt.selected_count === 0;
  }
  return receipt.constraint === "memory_on" && receipt.effective_memory_policy === receipt.routed_memory_policy;
}

function modelReceiptsValid(run) {
  if (!Array.isArray(run.configured_models) || !run.metrics || !Number.isInteger(run.metrics.model_responses) || run.metrics.model_responses <= 0) return false;
  if (!Array.isArray(run.model_receipts) || run.model_receipts.length !== run.metrics.model_responses) return false;
  return run.model_receipts.every((receipt) =>
    typeof receipt?.configured_model === "string" && run.configured_models.includes(receipt.configured_model) &&
    receipt.receipt_status === "observed" &&
    sha256Pattern.test(receipt.request_payload_sha256 || "") &&
    sha256Pattern.test(receipt.response_semantic_sha256 || "") &&
    sha256Pattern.test(receipt.provider_response_id_sha256 || "") &&
    !Object.hasOwn(receipt, "provider_response_id")
  );
}

function outputContractPassed(testCase, output) {
  const normalized = output.toLowerCase();
  return (testCase.verification?.output_contains || []).every((value) => normalized.includes(value.toLowerCase())) &&
    (testCase.verification?.output_not_contains || []).every((value) => !normalized.includes(value.toLowerCase()));
}

function toolSafetyContractPassed(testCase, run) {
  const allowed = new Set(testCase.verification?.allowed_tools || []);
  return allowed.size > 0 && Array.isArray(run.tool_receipts) &&
    run.tool_receipts.every((receipt) => receipt && typeof receipt.tool === "string" && allowed.has(receipt.tool));
}

function validateRunShape(run, cell, testCase) {
  const key = cell.key;
  requireFact(run.category === testCase.category, `${key}: category mismatch`);
  requireFact(typeof run.completed === "boolean" && typeof run.terminal_status === "string", `${key}: terminal receipt is missing`);
  requireFact(run.completed === (run.terminal_status === "completed"), `${key}: completed and terminal status disagree`);
  requireFact(run.product_mechanism_exercised === true, `${key}: product mechanism receipt is invalid`);
  requireFact(sha256Pattern.test(run.input_sha256 || ""), `${key}: input digest is invalid`);
  requireFact(typeof run.output === "string" && sha256Pattern.test(run.output_sha256 || ""), `${key}: output receipt is invalid`);
  requireFact(sha256(run.output) === run.output_sha256, `${key}: output digest mismatch`);
  requireFact(run.verification && typeof run.verification.answer_passed === "boolean" && typeof run.verification.quality_passed === "boolean", `${key}: verification receipt is invalid`);
  const answerPassed = outputContractPassed(testCase, run.output);
  requireFact(run.verification.answer_passed === answerPassed, `${key}: output contract receipt mismatch`);
  const toolSafetyPassed = toolSafetyContractPassed(testCase, run);
  requireFact(toolSafetyPassed || run.verification.external_effect_passed === false, `${key}: tool safety receipt mismatch`);
  if (typeof run.verification.external_effect_passed === "boolean") {
    requireFact(run.verification.quality_passed === (answerPassed && run.verification.external_effect_passed), `${key}: quality receipt is inconsistent`);
  } else {
    requireFact(run.verification.quality_passed === false, `${key}: unverifiable effect cannot pass quality`);
  }
  requireFact(run.metrics && Number.isInteger(run.metrics.model_responses) && run.metrics.model_responses >= 0, `${key}: model response count is invalid`);
  requireFact(Array.isArray(run.model_receipts), `${key}: model receipts are missing`);
  for (const receipt of run.model_receipts) {
    requireFact(receipt && typeof receipt === "object", `${key}: model receipt is invalid`);
    requireFact(!Object.hasOwn(receipt, "provider_response_id"), `${key}: raw provider response id leaked`);
    for (const field of ["request_payload_sha256", "response_semantic_sha256"]) {
      requireFact(sha256Pattern.test(receipt[field] || ""), `${key}: model ${field} is invalid`);
    }
  }
}

function validateRawIdentity(suite, raw, expectedSuiteSha256) {
  requireFact(raw?.schema === "cindx.agent-memory-effect-raw.v1", "raw schema mismatch");
  requireFact(raw.suite_id === suite.id && raw.suite_version === suite.version, "raw suite identity mismatch");
  requireFact(raw.suite_description === suite.description, "raw suite description mismatch");
  requireFact(sha256Pattern.test(raw.suite_sha256 || ""), "raw suite digest is invalid");
  if (expectedSuiteSha256) requireFact(raw.suite_sha256 === expectedSuiteSha256, "raw suite digest mismatch");
  requireFact(raw.execution_order_protocol === "cyclic_latin_square_v1", "raw execution-order protocol mismatch");
  requireFact(sha256Pattern.test(raw.execution_plan_sha256 || ""), "raw execution-plan digest is invalid");
  requireFact(sha256Pattern.test(raw.provider_config_sha256 || ""), "raw provider-config digest is invalid");
  requireFact(/^[0-9a-f]{40}$/.test(raw.git_commit || ""), "raw Git commit is invalid");
  requireFact(typeof raw.app_version === "string" && raw.app_version.length > 0, "raw app version is missing");
  requireFact(Number.isFinite(raw.generated_at_ms) && raw.generated_at_ms >= 0, "raw capture time is invalid");
  requireFact(typeof raw.provider_id === "string" && raw.provider_id.length > 0, "raw provider identity is missing");
  requireFact(typeof raw.provider_endpoint === "string" && URL.canParse(raw.provider_endpoint), "raw provider endpoint is invalid");
  requireFact(raw.configured_models && typeof raw.configured_models === "object" && !Array.isArray(raw.configured_models), "raw configured models are missing");
}

function pairedInvariant(on, off) {
  return on.input_sha256 === off.input_sha256 &&
    on.memory_seed_sha256 === off.memory_seed_sha256 &&
    JSON.stringify(on.configured_models) === JSON.stringify(off.configured_models) &&
    JSON.stringify(on.resolved_budget) === JSON.stringify(off.resolved_budget) &&
    on.strategy_receipt?.profile_sha256 === off.strategy_receipt?.profile_sha256 &&
    on.memory_evaluation_receipt?.routed_memory_policy === off.memory_evaluation_receipt?.routed_memory_policy &&
    on.memory_evaluation_receipt?.routed_query_sha256 === off.memory_evaluation_receipt?.routed_query_sha256 &&
    on.memory_evaluation_receipt?.non_memory_decision_sha256 === off.memory_evaluation_receipt?.non_memory_decision_sha256;
}

function qualityPassed(run) {
  return run.completed === true && run.verification?.quality_passed === true;
}

function runEvidenceValid(run) {
  return receiptValid(run) && modelReceiptsValid(run) &&
    Number.isInteger(run.memory_records_after_seed) && run.memory_records_after_seed > 0 &&
    sha256Pattern.test(run.memory_seed_sha256 || "") &&
    run.strategy_receipt && sha256Pattern.test(run.strategy_receipt.profile_sha256 || "") &&
    run.strategy_receipt.requested_policy === "auto_router" &&
    run.strategy_receipt.effective_policy === "single" &&
    run.strategy_receipt.execution_mode === "direct" &&
    run.strategy_receipt.decision_source === "matched_memory_evaluation" &&
    run.strategy_receipt.execution_constraint === "matched_memory_effect" &&
    run.strategy_receipt.workflow_profile_exercised === false &&
    !run.setup_failure && !run.evidence_error;
}

export function analyzeMemoryEffect(suite, raw, rawEvidenceSha256 = null, expectedSuiteSha256 = null) {
  validateSuite(suite);
  validateRawIdentity(suite, raw, expectedSuiteSha256);
  if (rawEvidenceSha256 !== null) requireFact(sha256Pattern.test(rawEvidenceSha256), "raw evidence digest is invalid");
  requireFact(raw.requested_replicates === 3, "raw replicate count mismatch");
  requireFact(JSON.stringify(raw.selected_cases) === JSON.stringify(suite.cases.map((item) => item.id)), "raw case selection mismatch");
  requireFact(JSON.stringify(raw.selected_treatments) === JSON.stringify(treatments), "raw treatment selection mismatch");
  requireFact(raw.runs?.length === 18, "raw report must retain all 18 cells");
  const cells = expectedCells(suite);
  const byKey = new Map();
  for (const run of raw.runs) {
    const key = `${run.case_id}/${run.treatment}/r${run.replicate}`;
    requireFact(!byKey.has(key), `duplicate run ${key}`);
    const expected = cells.find((cell) => cell.key === key);
    requireFact(expected, `unexpected run ${key}`);
    requireFact(run.execution_index === expected.executionIndex && run.treatment_position === expected.treatmentPosition, `${key}: execution receipt mismatch`);
    validateRunShape(run, expected, suite.cases.find((testCase) => testCase.id === expected.caseId));
    byKey.set(key, run);
  }
  requireFact(cells.every((cell) => byKey.has(cell.key)), "raw matrix is incomplete");

  const runRows = cells.map((cell) => {
    const run = byKey.get(cell.key);
    return {
      cell_sha256: digest("cindx.agent-memory-effect-cell.v1", cell.key),
      role: cell.role,
      treatment: cell.treatment,
      completed: run.completed === true,
      quality_passed: qualityPassed(run),
      evidence_valid: runEvidenceValid(run),
      recall_count: run.memory_evaluation_receipt?.recall_count ?? 0,
      selected_count: run.memory_evaluation_receipt?.selected_count ?? 0,
      routed_memory_policy: run.memory_evaluation_receipt?.routed_memory_policy ?? "missing",
      seed_sha256: run.memory_seed_sha256 ?? null,
      non_memory_decision_sha256: run.memory_evaluation_receipt?.non_memory_decision_sha256 ?? null
    };
  });

  const pairs = [];
  for (const testCase of suite.cases) {
    for (let replicate = 1; replicate <= 3; replicate += 1) {
      const on = byKey.get(`${testCase.id}/memory_on/r${replicate}`);
      const off = byKey.get(`${testCase.id}/memory_off/r${replicate}`);
      let status = "EVALUABLE";
      if (!runEvidenceValid(on) || !runEvidenceValid(off)) status = "INVALID_EVIDENCE";
      else if (!pairedInvariant(on, off)) status = "CONFOUNDED";
      else if (on.memory_evaluation_receipt.routed_memory_policy === "none" ||
        on.memory_evaluation_receipt.recall_count === 0 || on.memory_evaluation_receipt.selected_count === 0) status = "NOT_EXERCISED";
      pairs.push({
        pair_sha256: digest("cindx.agent-memory-effect-pair.v1", `${testCase.id}/r${replicate}`),
        case_sha256: digest("cindx.agent-memory-effect-case.v1", testCase.id),
        role: testCase.memory_effect.role,
        status,
        on_quality: qualityPassed(on),
        off_quality: qualityPassed(off),
        quality_delta: Number(qualityPassed(on)) - Number(qualityPassed(off))
      });
    }
  }

  const positive = pairs.filter((pair) => pair.role === "required");
  const negative = pairs.filter((pair) => pair.role === "irrelevant_control");
  const evidenceInvalid = runRows.filter((run) => !run.evidence_valid).length;
  const confounded = pairs.filter((pair) => pair.status === "CONFOUNDED").length;
  const notExercised = positive.filter((pair) => pair.status === "NOT_EXERCISED").length;
  const evaluablePositive = positive.filter((pair) => pair.status === "EVALUABLE");
  const evaluableNegative = negative.filter((pair) => pair.status === "EVALUABLE");
  const positiveRegressions = evaluablePositive.filter((pair) => pair.quality_delta < 0).length;
  const negativeRegressions = evaluableNegative.filter((pair) => pair.quality_delta < 0).length;
  const positiveImprovements = evaluablePositive.filter((pair) => pair.quality_delta > 0).length;
  const requiredCaseWins = Object.values(
    evaluablePositive.reduce((grouped, pair) => {
      grouped[pair.case_sha256] = (grouped[pair.case_sha256] ?? 0) + Number(pair.quality_delta > 0);
      return grouped;
    }, {})
  );
  const negativeReady = evaluableNegative.length === 3 && evaluableNegative.every((pair) => pair.on_quality && pair.off_quality);
  let decision = "NEUTRAL";
  if (evidenceInvalid > 0 || confounded > 0) decision = "INVALID_EVIDENCE";
  else if (positiveRegressions > 0 || negativeRegressions > 0) decision = "REGRESSED";
  else if (notExercised > 0 || evaluablePositive.length !== 6 || !negativeReady) decision = "NOT_EXERCISED";
  else if (positiveImprovements >= 4 && requiredCaseWins.length === 2 && requiredCaseWins.every((wins) => wins >= 2)) decision = "IMPROVED";

  const byTreatment = Object.fromEntries(treatments.map((treatment) => {
    const selected = runRows.filter((run) => run.treatment === treatment);
    return [treatment, {
      runs: selected.length,
      completed: selected.filter((run) => run.completed).length,
      quality_passed: selected.filter((run) => run.quality_passed).length,
      evidence_valid: selected.filter((run) => run.evidence_valid).length,
      recall_count: selected.reduce((sum, run) => sum + run.recall_count, 0),
      selected_count: selected.reduce((sum, run) => sum + run.selected_count, 0)
    }];
  }));
  const publicRuns = runRows.map(({ seed_sha256, non_memory_decision_sha256, ...run }) => run);
  return {
    schema: "cindx.agent-memory-effect-sanitized.v1",
    suite_sha256: raw.suite_sha256,
    execution_plan_sha256: raw.execution_plan_sha256,
    raw_evidence_sha256: rawEvidenceSha256,
    git_commit: raw.git_commit,
    app_version: raw.app_version,
    provider_binding_sha256: digestJson("cindx.agent-memory-effect-provider-binding.v1", {
      provider_id: raw.provider_id,
      provider_endpoint: raw.provider_endpoint,
      configured_models: raw.configured_models
    }),
    provider_config_sha256: raw.provider_config_sha256,
    evidence_digest: digestJson("cindx.agent-memory-effect-sanitized-evidence.v1", { runRows, pairs }),
    denominator: { cells: 18, pairs: 9, required_pairs: 6, irrelevant_control_pairs: 3 },
    outcomes: { by_treatment: byTreatment, terminal_status_counts: counts(raw.runs.map((run) => run.terminal_status)) },
    pair_status_counts: counts(pairs.map((pair) => pair.status)),
    effect: { positive_improvements: positiveImprovements, positive_regressions: positiveRegressions, negative_regressions: negativeRegressions },
    confounds: { evidence_invalid_cells: evidenceInvalid, confounded_pairs: confounded, required_not_exercised_pairs: notExercised },
    decision,
    runs: publicRuns,
    pairs
  };
}

export function renderMemoryEffectMarkdown(report) {
  return `# Cindx memory-effect evaluation\n\n- Decision: **${report.decision}**\n- Frozen denominator: ${report.denominator.cells} cells / ${report.denominator.pairs} matched pairs\n- Pair states: ${JSON.stringify(report.pair_status_counts)}\n- Positive improvements/regressions: ${report.effect.positive_improvements}/${report.effect.positive_regressions}\n- Negative-control regressions: ${report.effect.negative_regressions}\n- Evidence digest: \`${report.evidence_digest}\`\n\nFailures, invalid evidence, confounds, and non-exercised positive routes remain in the frozen denominator.\n`;
}

function write(file, content) {
  fs.mkdirSync(path.dirname(path.resolve(file)), { recursive: true });
  fs.writeFileSync(file, content);
}

function main(argv) {
  const options = parseArguments(argv);
  const suiteBytes = fs.readFileSync(options.suite);
  const rawBytes = fs.readFileSync(options.raw);
  const suite = JSON.parse(suiteBytes);
  const raw = JSON.parse(rawBytes);
  requireFact(raw.suite_sha256 === crypto.createHash("sha256").update(suiteBytes).digest("hex"), "raw suite digest mismatch");
  const suiteSha256 = sha256(suiteBytes);
  requireFact(raw.suite_sha256 === suiteSha256, "raw suite digest mismatch");
  const report = analyzeMemoryEffect(suite, raw, sha256(rawBytes), suiteSha256);
  write(options.sanitized, `${JSON.stringify(report, null, 2)}\n`);
  write(options.markdown, renderMemoryEffectMarkdown(report));
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`Agent memory-effect analysis failed: ${error.message}\n`);
    process.exitCode = 1;
  }
}
