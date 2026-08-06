import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import test from "node:test";
import { analyzeMemoryEffect } from "./agent-memory-effect-contract.mjs";
import { validatePreflight } from "./run-agent-realworld.mjs";

const suite = JSON.parse(fs.readFileSync(new URL("../benchmarks/agent/memory-effect-v1.json", import.meta.url)));

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function passingOutput(testCase) {
  return (testCase.verification.output_contains || []).join("; ");
}

function setRunOutput(run, output, externalEffectPassed = true) {
  const testCase = suite.cases.find((item) => item.id === run.case_id);
  const normalized = output.toLowerCase();
  const answerPassed = (testCase.verification.output_contains || []).every((value) => normalized.includes(value.toLowerCase())) &&
    (testCase.verification.output_not_contains || []).every((value) => !normalized.includes(value.toLowerCase()));
  run.output = output;
  run.output_sha256 = sha256(output);
  run.verification.answer_passed = answerPassed;
  run.verification.external_effect_passed = externalEffectPassed;
  run.verification.quality_passed = answerPassed && externalEffectPassed;
}

test("no-call runner preflight recognizes only the frozen 18-cell protocol", () => {
  assert.deepEqual(
    validatePreflight({ suite, gitHead: "a".repeat(40), status: "" }),
    { replicates: 3 }
  );
  assert.throws(
    () => validatePreflight({ suite, gitHead: "a".repeat(40), status: "", requestedReplicates: "1" }),
    /exactly three replicates/
  );
});

function rawFixture() {
  const runs = [];
  let block = 0;
  for (let replicate = 1; replicate <= 3; replicate += 1) {
    for (const testCase of suite.cases) {
      const rotation = block % 2;
      for (let position = 0; position < 2; position += 1) {
        const treatment = suite.treatments[(position + rotation) % 2];
        const required = testCase.memory_effect.role === "required";
        const qualityPassed = treatment === "memory_on" || !required;
        const output = qualityPassed ? passingOutput(testCase) : "The requested values are unavailable.";
        runs.push({
          execution_index: runs.length + 1,
          treatment_position: position + 1,
          replicate,
          case_id: testCase.id,
          category: testCase.category,
          treatment,
          product_mechanism_exercised: true,
          completed: true,
          terminal_status: "completed",
          configured_models: ["private-model"],
          memory_records_after_seed: 1,
          memory_seed_sha256: "a".repeat(64),
          input_sha256: "b".repeat(64),
          evidence_error: null,
          setup_failure: null,
          output,
          output_sha256: sha256(output),
          resolved_budget: { max_duration_ms: 1 },
          strategy_receipt: {
            requested_policy: "auto_router",
            effective_policy: "single",
            execution_mode: "direct",
            decision_source: "matched_memory_evaluation",
            execution_constraint: "matched_memory_effect",
            workflow_profile_exercised: false,
            profile_sha256: "c".repeat(64)
          },
          tool_receipts: [],
          metrics: { model_responses: 1 },
          model_receipts: [{
            configured_model: "private-model",
            request_payload_sha256: "6".repeat(64),
            response_semantic_sha256: "7".repeat(64),
            provider_response_id_sha256: "8".repeat(64),
            receipt_status: "observed"
          }],
          memory_evaluation_receipt: {
            constraint: treatment,
            routed_memory_policy: "relevant",
            effective_memory_policy: treatment === "memory_off" ? "none" : "relevant",
            recall_count: treatment === "memory_on" ? 1 : 0,
            selected_count: treatment === "memory_on" ? 1 : 0,
            routed_query_sha256: "d".repeat(64),
            selected_memory_ids_sha256: "e".repeat(64),
            non_memory_decision_sha256: "f".repeat(64)
          },
          verification: {
            answer_passed: qualityPassed,
            external_effect_passed: true,
            quality_passed: qualityPassed
          }
        });
      }
      block += 1;
    }
  }
  return {
    schema: "cindx.agent-memory-effect-raw.v1",
    suite_id: suite.id,
    suite_version: suite.version,
    suite_description: suite.description,
    requested_replicates: 3,
    selected_cases: suite.cases.map((item) => item.id),
    selected_treatments: [...suite.treatments],
    suite_sha256: "1".repeat(64),
    execution_plan_sha256: "2".repeat(64),
    execution_order_protocol: "cyclic_latin_square_v1",
    generated_at_ms: 1,
    git_commit: "3".repeat(40),
    app_version: "0.0.0",
    provider_id: "private-provider",
    provider_endpoint: "https://private.example/v1",
    configured_models: { default: "private-model" },
    provider_config_sha256: "4".repeat(64),
    runs
  };
}

test("matched 18-cell evidence can demonstrate bounded memory benefit", () => {
  const report = analyzeMemoryEffect(suite, rawFixture(), "5".repeat(64));
  assert.equal(report.decision, "IMPROVED");
  assert.deepEqual(report.denominator, { cells: 18, pairs: 9, required_pairs: 6, irrelevant_control_pairs: 3 });
  assert.equal(report.outcomes.by_treatment.memory_off.recall_count, 0);
  const encoded = JSON.stringify(report);
  for (const secret of ["private-provider", "private.example", "private-model", "Red Juniper"]) {
    assert.equal(encoded.includes(secret), false);
  }
});

test("routed-none positive cells are not exercised rather than counted as wins", () => {
  const raw = rawFixture();
  for (const run of raw.runs.filter((item) => item.case_id === suite.cases[0].id)) {
    run.memory_evaluation_receipt.routed_memory_policy = "none";
    run.memory_evaluation_receipt.effective_memory_policy = "none";
    run.memory_evaluation_receipt.recall_count = 0;
    run.memory_evaluation_receipt.selected_count = 0;
  }
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "NOT_EXERCISED");
  assert.equal(report.pair_status_counts.NOT_EXERCISED, 3);
});

test("an irrelevant-memory control must actually recall its decoy", () => {
  const raw = rawFixture();
  for (const run of raw.runs.filter((item) => item.case_id === suite.cases[2].id)) {
    run.memory_evaluation_receipt.routed_memory_policy = "none";
    run.memory_evaluation_receipt.effective_memory_policy = "none";
    run.memory_evaluation_receipt.recall_count = 0;
    run.memory_evaluation_receipt.selected_count = 0;
  }
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "NOT_EXERCISED");
  assert.equal(report.pair_status_counts.NOT_EXERCISED, 3);
});

test("an effectful tool attempt cannot pass the frozen read-only contract", () => {
  const raw = rawFixture();
  const run = raw.runs[0];
  run.tool_receipts.push({ tool: "file.write", status: "denied" });
  assert.throws(() => analyzeMemoryEffect(suite, raw), /tool safety receipt mismatch/);
});

test("memory-off leakage invalidates evidence without removing failures from denominator", () => {
  const raw = rawFixture();
  const leaked = raw.runs.find((run) => run.treatment === "memory_off");
  leaked.memory_evaluation_receipt.recall_count = 1;
  leaked.memory_evaluation_receipt.selected_count = 1;
  leaked.completed = false;
  leaked.terminal_status = "failed";
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "INVALID_EVIDENCE");
  assert.equal(report.denominator.cells, 18);
  assert.equal(report.outcomes.terminal_status_counts.failed, 1);
  assert.equal(report.confounds.evidence_invalid_cells, 1);
});

test("provider or task failure with complete receipts remains a quality regression", () => {
  const raw = rawFixture();
  const timedOut = raw.runs.find((run) =>
    run.treatment === "memory_on" && run.case_id === suite.cases[0].id && run.replicate === 1
  );
  timedOut.completed = false;
  timedOut.terminal_status = "timed_out";
  setRunOutput(timedOut, "The provider timed out before producing an answer.");
  const pairedOff = raw.runs.find((run) =>
    run.treatment === "memory_off" && run.case_id === suite.cases[0].id && run.replicate === 1
  );
  setRunOutput(pairedOff, passingOutput(suite.cases[0]));
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "REGRESSED");
  assert.equal(report.confounds.evidence_invalid_cells, 0);
  assert.equal(report.outcomes.terminal_status_counts.timed_out, 1);
  assert.equal(report.denominator.cells, 18);
});

test("partial output cannot score as quality when the run did not complete", () => {
  const raw = rawFixture();
  const partial = raw.runs.find((run) =>
    run.treatment === "memory_on" && run.case_id === suite.cases[0].id && run.replicate === 1
  );
  partial.completed = false;
  partial.terminal_status = "timed_out";
  assert.equal(partial.verification.quality_passed, true);
  const pairedOff = raw.runs.find((run) =>
    run.treatment === "memory_off" && run.case_id === suite.cases[0].id && run.replicate === 1
  );
  setRunOutput(pairedOff, passingOutput(suite.cases[0]));
  const report = analyzeMemoryEffect(suite, raw);
  const row = report.runs.find((run) => run.cell_sha256 === sha256(`cindx.agent-memory-effect-cell.v1\0${suite.cases[0].id}/memory_on/r1`));
  assert.equal(row.quality_passed, false);
  assert.equal(report.decision, "REGRESSED");
});

test("one weak positive win stays neutral", () => {
  const raw = rawFixture();
  for (const run of raw.runs) {
    const testCase = suite.cases.find((item) => item.id === run.case_id);
    setRunOutput(run, passingOutput(testCase));
  }
  const weakWin = raw.runs.find((run) =>
    run.treatment === "memory_off" && run.case_id === suite.cases[0].id && run.replicate === 1
  );
  setRunOutput(weakWin, "The requested values are unavailable.");
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.effect.positive_improvements, 1);
  assert.equal(report.decision, "NEUTRAL");
});

test("missing provider receipt coverage invalidates evidence", () => {
  const raw = rawFixture();
  raw.runs[0].model_receipts = [];
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "INVALID_EVIDENCE");
  assert.equal(report.confounds.evidence_invalid_cells, 1);
});

test("empty seed receipt plus setup and evidence errors invalidate evidence", () => {
  const raw = rawFixture();
  raw.runs[0].memory_records_after_seed = 0;
  raw.runs[1].setup_failure = { stage: "memory_seed", code: "data", retryable: false };
  raw.runs[2].evidence_error = "provider receipt coverage is incomplete";
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.decision, "INVALID_EVIDENCE");
  assert.equal(report.confounds.evidence_invalid_cells, 3);
});

test("raw output digest and output-contract claims are recomputed", () => {
  const digestTamper = rawFixture();
  digestTamper.runs[0].output += "tamper";
  assert.throws(() => analyzeMemoryEffect(suite, digestTamper), /output digest mismatch/);

  const answerTamper = rawFixture();
  const run = answerTamper.runs.find((item) => item.treatment === "memory_on");
  run.output = "Red Juniper";
  run.output_sha256 = sha256(run.output);
  assert.throws(() => analyzeMemoryEffect(suite, answerTamper), /output contract receipt mismatch/);

  const decoyTamper = rawFixture();
  const control = suite.cases.find((item) => item.memory_effect.role === "irrelevant_control");
  const decoy = decoyTamper.runs.find((item) => item.case_id === control.id);
  decoy.output = `${passingOutput(control)}; Red Juniper`;
  decoy.output_sha256 = sha256(decoy.output);
  assert.throws(() => analyzeMemoryEffect(suite, decoyTamper), /output contract receipt mismatch/);
});

test("raw identity and required digests fail closed", () => {
  const raw = rawFixture();
  raw.execution_plan_sha256 = "invalid";
  assert.throws(() => analyzeMemoryEffect(suite, raw), /execution-plan digest/);
  const suiteMismatch = rawFixture();
  assert.throws(
    () => analyzeMemoryEffect(suite, suiteMismatch, null, "9".repeat(64)),
    /suite digest mismatch/
  );
});

test("paired route policy and query digest mutations are confounded", async (t) => {
  for (const [field, value, effectivePolicy] of [
    ["routed_memory_policy", "comprehensive", "comprehensive"],
    ["routed_query_sha256", "0".repeat(64), null]
  ]) {
    await t.test(field, () => {
      const raw = rawFixture();
      const mutated = raw.runs.find((run) =>
        run.treatment === "memory_on" && run.case_id === suite.cases[0].id && run.replicate === 1
      );
      mutated.memory_evaluation_receipt[field] = value;
      if (effectivePolicy) mutated.memory_evaluation_receipt.effective_memory_policy = effectivePolicy;
      const report = analyzeMemoryEffect(suite, raw);
      assert.equal(report.decision, "INVALID_EVIDENCE");
      assert.equal(report.confounds.confounded_pairs, 1);
    });
  }
});

test("regression takes precedence over a separate not-exercised pair", () => {
  const raw = rawFixture();
  for (const run of raw.runs.filter((item) => item.case_id === suite.cases[0].id)) {
    run.memory_evaluation_receipt.routed_memory_policy = "none";
    run.memory_evaluation_receipt.effective_memory_policy = "none";
    run.memory_evaluation_receipt.recall_count = 0;
    run.memory_evaluation_receipt.selected_count = 0;
  }
  const control = suite.cases.find((item) => item.memory_effect.role === "irrelevant_control");
  const failedOn = raw.runs.find((run) =>
    run.case_id === control.id && run.treatment === "memory_on" && run.replicate === 1
  );
  setRunOutput(failedOn, "No signing policy was found.");
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.pair_status_counts.NOT_EXERCISED, 3);
  assert.equal(report.effect.negative_regressions, 1);
  assert.equal(report.decision, "REGRESSED");
});

test("negative controls must all be evaluable and pass both arms", () => {
  const raw = rawFixture();
  const control = suite.cases.find((item) => item.memory_effect.role === "irrelevant_control");
  for (const run of raw.runs.filter((item) => item.case_id === control.id)) {
    setRunOutput(run, "No signing policy was found.");
  }
  const report = analyzeMemoryEffect(suite, raw);
  assert.equal(report.effect.negative_regressions, 0);
  assert.equal(report.decision, "NOT_EXERCISED");
});
