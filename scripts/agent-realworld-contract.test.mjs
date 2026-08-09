import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { renderMarkdown, validateAndSanitize } from "./agent-realworld-contract.mjs";
import {
  evaluationEnvironment,
  evaluationPlan,
  mergeRunCheckpoint,
  normalizeInterruptedRun,
  parseArguments,
  preparePlaywrightResource,
  runKey,
  validateCheckpoint,
  validateProfileArtifact,
  validateOutputPaths,
  validatePreflight
} from "./run-agent-realworld.mjs";

function hash(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fixture({ current = false } = {}) {
  const treatments = current
    ? ["oracle_reference", "grounded_direct", "auto", "pro"]
    : ["direct", "fast", "auto", "pro"];
  const oracleTreatment = current ? "oracle_reference" : "direct";
  const isOracle = (treatment) => treatment === oracleTreatment;
  const suite = {
    schema: current ? "cindx.agent-realworld-suite.v4" : "cindx.agent-realworld-suite.v3",
    id: "fixture",
    version: current ? 5 : 4,
    description: "fixture",
    default_replicates: 1,
    per_run_timeout_seconds: 600,
    execution_order: {
      protocol: "cyclic_latin_square_v1",
      base_treatments: treatments
    },
    ...(current
      ? {
          claim_contract_v2: {
            schema: "cindx.agent-realworld-claims.v2",
            baseline: "grounded_direct",
            oracle_reference: "oracle_reference",
            candidates: {
              auto: {
                budget_relation: "iso_budget",
                maximum_median_latency_ratio: 3,
                maximum_total_token_ratio: 4
              },
              pro: {
                budget_relation: "descriptive_only",
                maximum_median_latency_ratio: 6,
                maximum_total_token_ratio: 8
              }
            },
            minimum_quality_delta: 0,
            minimum_completion_delta: 0,
            minimum_any_quality_improvement_runs: 1,
            maximum_setup_failures: 0,
            maximum_safety_violations: 0
          }
        }
      : {
          promotion_v1: {
            schema: "cindx.agent-realworld-promotion.v1",
            baseline: "fast",
            candidates: {
              auto: { maximum_median_latency_ratio: 3, maximum_total_token_ratio: 4 },
              pro: { maximum_median_latency_ratio: 6, maximum_total_token_ratio: 8 }
            },
            minimum_quality_delta: 0,
            minimum_completion_delta: 0,
            minimum_any_improvement_runs: 1,
            maximum_setup_failures: 0,
            maximum_safety_violations: 0
          }
        }),
    treatments,
    cases: [
      {
        id: "case-a",
        category: "coding",
        verification: {
          required_tools_any: ["file.read"],
          required_tools_all: []
        }
      }
    ]
  };
  const suiteBytes = Buffer.from(JSON.stringify(suite));
  const output = "private answer";
  const profileArtifacts = {
    auto: {
      mode: "fresh_seed",
      path: null,
      file_sha256: null,
      artifact_sha256: null,
      candidate_sha256: null,
      evolution_method: null
    },
    pro: {
      mode: "fresh_seed",
      path: null,
      file_sha256: null,
      artifact_sha256: null,
      candidate_sha256: null,
      evolution_method: null
    }
  };
  const gitCommit = "b".repeat(40);
  const providerConfigSha256 = "c".repeat(64);
  const providerBinding = {
    provider_id: "fixture",
    provider_endpoint: "https://user:secret@example.test/v1?key=private",
    configured_models: { default: "model-a" }
  };
  const plan = evaluationPlan(
    {
      suite,
      suiteSha256: hash(suiteBytes),
      gitHead: gitCommit,
      replicates: 1,
      providerBinding,
      providerConfigSha256,
      profileArtifacts
    },
    {}
  );
  const resolvedBudget = {
    max_duration_ms: 600_000,
    model_call_timeout_ms: 60_000,
    tool_call_timeout_ms: 30_000,
    initial_model_calls: 1,
    max_model_calls: 4,
    initial_tool_calls: 1,
    max_tool_calls: 4,
    no_progress_timeout_ms: 60_000,
    max_total_tokens: 32_000,
    max_physical_model_attempts: 6,
    terminal_token_reserve: 1_000,
    terminal_physical_model_attempt_reserve: 1
  };
  const runs = plan.entries.map((entry) => ({
    replicate: 1,
    case_id: "case-a",
    category: "coding",
    treatment: entry.treatment,
    execution_index: entry.executionIndex,
    treatment_position: entry.treatmentPosition,
    product_mechanism_exercised: !isOracle(entry.treatment),
    completed: true,
    terminal_status: "completed",
    evidence_error: null,
    configured_models: ["model-a"],
    tools_used: isOracle(entry.treatment) ? [] : ["file.read"],
    fixture_receipt: null,
    tool_receipts:
      isOracle(entry.treatment)
        ? []
        : [
            {
              call_sha256: hash(`call-${entry.executionIndex}`),
              tool: "file.read",
              status: "succeeded",
              input_fingerprint: hash(`input-${entry.executionIndex}`),
              started_sequence: 1,
              finished_sequence: 2,
              target_sha256: null,
              evidence_sha256: null,
              artifacts: []
            }
          ],
    memory_records_after_seed: null,
    input_sha256: "a".repeat(64),
    output_sha256: hash(output),
    output,
    error: null,
    setup_failure: null,
    resolved_budget: { ...resolvedBudget },
    strategy_receipt:
      isOracle(entry.treatment)
        ? null
        : {
            requested_policy: entry.treatment === "grounded_direct" ? "auto" : entry.treatment,
            effective_policy: entry.treatment === "grounded_direct" ? "single" : entry.treatment,
            execution_mode: "direct",
            decision_source: "fixture",
            ...(current
              ? {
                  execution_constraint:
                    entry.treatment === "grounded_direct" ? "grounded_direct" : "native"
                }
              : {}),
            decision_sha256: hash(`decision-${entry.executionIndex}`),
            routing_signature_sha256: hash(`routing-${entry.executionIndex}`),
            route_profile_sha256: hash("seed-route-phenotype"),
            profile_source: "built_in_seed",
            profile_id:
              entry.treatment === "grounded_direct" ? "auto-seed" : `${entry.treatment}-seed`,
            profile_sha256: hash(
              entry.treatment === "grounded_direct" ? "auto-seed" : `${entry.treatment}-seed`
            ),
            profile_generation: 0,
            parent_profile_ids: [],
            learned_artifact_sha256: null,
            learned_method: null,
            stable_profile_id: null,
            dataset_sha256: null,
            paired_evidence_sha256: null,
            promotion_gate_protocol: null,
            workflow_profile_exercised: false,
            route_profile_semantics_exercised: false
          },
    model_receipts: [
      {
        configured_model: "model-a",
        provider_response_model: "model-a",
        provider_response_id_sha256: hash(`provider-${entry.executionIndex}`),
        provider_system_fingerprint_sha256: null,
        request_payload_sha256: hash(`request-${entry.executionIndex}`),
        response_semantic_sha256: hash(output),
        receipt_status: "observed"
      }
    ],
    metrics: {
      latency_ms: ["fast", "grounded_direct"].includes(entry.treatment) ? 10 : 20,
      setup_latency_ms: 0,
      model_calls: 1,
      model_responses: 1,
      tool_calls: isOracle(entry.treatment) ? 0 : 1,
      tool_succeeded: isOracle(entry.treatment) ? 0 : 1,
      tool_failed: 0,
      tool_cancelled: 0,
      tool_denied: 0,
      tool_incomplete: 0,
      tool_superseded: 0,
      tool_invalid: 0,
      permission_requests: 0,
      denied_permissions: 0,
      recovery_events: 0,
      prompt_tokens: 5,
      completion_tokens: 3,
      total_tokens: 8,
      context_tokens_used: 5,
      resident_kib_after: 100,
      workspace_bytes_after: 50
    },
    verification: {
      quality_passed: true,
      answer_passed: true,
      external_effect_passed: isOracle(entry.treatment) ? null : true,
      passed_checks: 1,
      total_checks: 1,
      safety_violations: 0,
      failures: [],
      expected_browser_target_sha256: null,
      postcondition_receipts: []
    }
  }));
  const raw = {
    schema: current ? "cindx.agent-realworld-raw.v4" : "cindx.agent-realworld-raw.v3",
    suite_id: suite.id,
    suite_version: suite.version,
    suite_description: suite.description,
    suite_sha256: hash(suiteBytes),
    generated_at_ms: 1,
    git_commit: gitCommit,
    app_version: "0.1.82",
    provider_id: "fixture",
    provider_endpoint: "https://user:secret@example.test/v1?key=private",
    configured_models: { default: "model-a" },
    provider_config_sha256: providerConfigSha256,
    requested_replicates: 1,
    selected_cases: ["case-a"],
    selected_treatments: suite.treatments,
    execution_order_protocol: plan.protocol,
    execution_plan_sha256: plan.sha256,
    profile_artifacts: plan.profileArtifacts,
    runs
  };
  const rawBytes = Buffer.from(JSON.stringify(raw));
  return { suite, suiteBytes, raw, rawBytes };
}

function browserFixture() {
  const value = fixture();
  const body = "<!doctype html><title>Fixture</title><p>incident evidence</p>";
  const targetUrl = "http://127.0.0.1:41731/fixture/case-a/site/index.html";
  const targetSha256 = hash(targetUrl);
  const bodySha256 = hash(body);
  value.suite.cases = [
    {
      id: "case-a",
      category: "browser",
      files: [{ path: "site/index.html", content: body }],
      verification: {
        required_tools_any: ["browser.extract_text"],
        required_tools_all: ["browser.open"],
        browser_target_receipt: {
          tools_all: ["browser.open"],
          tools_any: ["browser.extract_text"],
          minimum_artifacts: 1
        }
      }
    }
  ];
  value.suiteBytes = Buffer.from(JSON.stringify(value.suite));
  value.raw.suite_sha256 = hash(value.suiteBytes);
  const profileArtifacts = {
    auto: { ...value.raw.profile_artifacts.auto, path: null },
    pro: { ...value.raw.profile_artifacts.pro, path: null }
  };
  const plan = evaluationPlan(
    {
      suite: value.suite,
      suiteSha256: value.raw.suite_sha256,
      gitHead: value.raw.git_commit,
      replicates: 1,
      providerBinding: {
        provider_id: value.raw.provider_id,
        provider_endpoint: value.raw.provider_endpoint,
        configured_models: value.raw.configured_models
      },
      providerConfigSha256: value.raw.provider_config_sha256,
      profileArtifacts
    },
    {}
  );
  value.raw.execution_plan_sha256 = plan.sha256;
  for (const run of value.raw.runs) {
    run.category = "browser";
    run.fixture_receipt = {
      target_sha256: targetSha256,
      body_sha256: bodySha256,
      successful_requests: run.treatment === "direct" ? 0 : 1
    };
    run.verification.expected_browser_target_sha256 =
      run.treatment === "direct" ? null : targetSha256;
    if (run.treatment === "direct") continue;
    run.tools_used = ["browser.open", "browser.extract_text"];
    run.tool_receipts = [
      {
        call_sha256: hash(`browser-open-${run.execution_index}`),
        tool: "browser.open",
        status: "succeeded",
        input_fingerprint: hash(`browser-open-input-${run.execution_index}`),
        started_sequence: 1,
        finished_sequence: 2,
        target_sha256: targetSha256,
        evidence_sha256: null,
        artifacts: []
      },
      {
        call_sha256: hash(`browser-evidence-${run.execution_index}`),
        tool: "browser.extract_text",
        status: "succeeded",
        input_fingerprint: hash(`browser-evidence-input-${run.execution_index}`),
        started_sequence: 3,
        finished_sequence: 4,
        target_sha256: targetSha256,
        evidence_sha256: hash("browser evidence"),
        artifacts: [
          {
            path_sha256: hash("artifacts/browser-evidence.txt"),
            content_sha256: hash("browser evidence"),
            bytes: 16,
            mime_type: "text/plain"
          }
        ]
      }
    ];
    run.metrics.tool_calls = 2;
    run.metrics.tool_succeeded = 2;
  }
  value.rawBytes = Buffer.from(JSON.stringify(value.raw));
  return { ...value, targetUrl, targetSha256 };
}

test("validates the complete matrix and removes private output", () => {
  const report = validateAndSanitize(fixture());
  assert.equal(report.schema, "cindx.agent-realworld-sanitized.v3");
  assert.equal(report.decision.status, "VALID_BASELINE");
  assert.equal(report.aggregates.fast.quality_pass_rate, 1);
  assert.equal(report.paired_against_fast.pro.quality_pass_delta, 0);
  assert.equal(report.suite.per_run_timeout_seconds, 600);
  assert.equal(report.outcomes.completed_runs, 4);
  assert.equal(report.outcomes.non_completed_runs, 0);
  assert.equal(report.decision.uplift_status, "NO_GO");
  assert.equal(report.runs[0].output, undefined);
  assert.equal(report.runs[0].error, undefined);
  assert.equal(report.evidence.provider_endpoint, "https://example.test/v1");
  assert.deepEqual(report.evidence.provider_identity, {
    status: "hashed_provider_id",
    sha256: hash("fixture")
  });
  assert.equal(report.decision.learned_profile_status, "FRESH_SEED_ONLY");
  const markdown = renderMarkdown(report);
  assert.match(markdown, /\| Treatment \| Complete \| Quality \| External effect \| Safety violations \|/);
  assert.match(markdown, /\| coding \| 100\.0% \| 100\.0% \| 100\.0% \| 100\.0% \|/);
  assert.match(markdown, /Capture time: `1970-01-01T00:00:00\.001Z`/);
  assert.match(markdown, /## Treatment Configuration and Budget/);
  assert.match(markdown, /## Failure and Permission Outcomes/);
  assert.match(markdown, /## Paired Against Fast/);
  assert.match(markdown, /## Confounds/);
  assert.match(markdown, /Broad orchestration uplift: \*\*NO-GO\*\*/);
  assert.match(markdown, /fresh-seed quality evidence/i);
  assert.doesNotMatch(markdown, /private answer|secret|key=private/);
});

test("separates current grounded-direct mechanism claims without inventing learning uplift", () => {
  const report = validateAndSanitize(fixture({ current: true }));
  assert.equal(report.schema, "cindx.agent-realworld-sanitized.v4");
  assert.equal(report.decision.status, "VALID_BASELINE");
  assert.equal(report.decision.mechanism_claims.adaptive_direct.state, "NEUTRAL");
  assert.equal(report.decision.mechanism_claims.adaptive_direct.denominator, 1);
  assert.equal(report.decision.mechanism_claims.workflow.state, "NOT_EXERCISED");
  assert.equal(report.decision.mechanism_claims.learned_profile.state, "NOT_EXERCISED");
  assert.equal(report.decision.mechanism_claims.distillation.state, "NOT_EXERCISED");
  assert.equal(report.paired_against_baseline.auto.pairs, 1);
  assert.equal(report.evidence.oracle_reference_runs, 1);
  const markdown = renderMarkdown(report);
  assert.match(markdown, /Paired Against Grounded Direct/);
  assert.doesNotMatch(markdown, /Broad orchestration uplift/);
});

test("binds browser success to the exact HTTP fixture target and artifact evidence", () => {
  const browser = browserFixture();
  const fast = browser.raw.runs.find((run) => run.treatment === "fast");
  fast.output = `private ${browser.targetUrl} /private/fixture/site/index.html`;
  fast.output_sha256 = hash(fast.output);
  browser.rawBytes = Buffer.from(JSON.stringify(browser.raw));
  const report = validateAndSanitize(browser);
  const sanitized = JSON.stringify(report);
  const sanitizedFast = report.runs.find((run) => run.treatment === "fast");
  assert.equal(sanitizedFast.fixture_receipt.target_sha256, browser.targetSha256);
  assert.equal(sanitizedFast.tool_receipts[1].artifacts.length, 1);
  assert.doesNotMatch(sanitized, /127\.0\.0\.1|\/private\/fixture|site\/index\.html/);

  const wrongTarget = browserFixture();
  wrongTarget.raw.runs.find((run) => run.treatment === "fast").tool_receipts[1].target_sha256 =
    hash("http://127.0.0.1:41731/wrong");
  wrongTarget.rawBytes = Buffer.from(JSON.stringify(wrongTarget.raw));
  assert.throws(
    () => validateAndSanitize(wrongTarget),
    /browser evidence lacks an exact-target artifact receipt/
  );

  const missingArtifact = browserFixture();
  missingArtifact.raw.runs.find(
    (run) => run.treatment === "auto"
  ).tool_receipts[1].artifacts = [];
  missingArtifact.rawBytes = Buffer.from(JSON.stringify(missingArtifact.raw));
  assert.throws(
    () => validateAndSanitize(missingArtifact),
    /browser evidence lacks an exact-target artifact receipt/
  );

  const tamperedBody = browserFixture();
  tamperedBody.raw.runs.find((run) => run.treatment === "pro").fixture_receipt.body_sha256 =
    hash("tampered");
  tamperedBody.rawBytes = Buffer.from(JSON.stringify(tamperedBody.raw));
  assert.throws(() => validateAndSanitize(tamperedBody), /HTTP fixture body hash mismatch/);
});

test("counts failed tool receipts without letting them satisfy required tools", () => {
  const forged = fixture();
  const fast = forged.raw.runs.find((run) => run.treatment === "fast");
  fast.tool_receipts[0].status = "failed";
  fast.tools_used = [];
  fast.metrics.tool_succeeded = 0;
  fast.metrics.tool_failed = 1;
  forged.rawBytes = Buffer.from(JSON.stringify(forged.raw));
  assert.throws(() => validateAndSanitize(forged), /required tools lack successful receipts/);

  fast.verification.quality_passed = false;
  fast.verification.external_effect_passed = false;
  forged.rawBytes = Buffer.from(JSON.stringify(forged.raw));
  const report = validateAndSanitize(forged);
  assert.equal(report.aggregates.fast.runs, 1);
  assert.equal(report.aggregates.fast.tool_calls, 1);
  assert.equal(report.aggregates.fast.tool_succeeded, 0);
  assert.equal(report.aggregates.fast.tool_failed, 1);
  assert.equal(report.aggregates.fast.quality_pass_rate, 0);
});

test("keeps a timed-out run with no successful tool receipt in the denominator", () => {
  const timedOut = fixture();
  const auto = timedOut.raw.runs.find((run) => run.treatment === "auto");
  auto.completed = false;
  auto.terminal_status = "timed_out";
  auto.error = "frozen deadline";
  auto.tools_used = [];
  auto.tool_receipts = [];
  auto.metrics.tool_calls = 0;
  auto.metrics.tool_succeeded = 0;
  auto.verification.quality_passed = false;
  auto.verification.answer_passed = false;
  auto.verification.external_effect_passed = false;
  auto.verification.passed_checks = 0;
  auto.verification.total_checks = 1;
  auto.verification.postcondition_receipts = [];
  auto.verification.failures = ["run did not reach verification"];
  timedOut.rawBytes = Buffer.from(JSON.stringify(timedOut.raw));

  const report = validateAndSanitize(timedOut);
  assert.equal(report.runs.length, 4);
  assert.equal(report.outcomes.timeouts, 1);
  assert.equal(report.aggregates.auto.runs, 1);
  assert.equal(report.aggregates.auto.tool_calls, 0);
  assert.equal(report.aggregates.auto.quality_pass_rate, 0);
});

test("binds completion strictly to the completed terminal status", () => {
  const falseCompletion = fixture();
  falseCompletion.raw.runs.find((run) => run.treatment === "auto").terminal_status =
    "timed_out";
  falseCompletion.rawBytes = Buffer.from(JSON.stringify(falseCompletion.raw));
  assert.throws(
    () => validateAndSanitize(falseCompletion),
    /completed flag and terminal status disagree/
  );

  const falseTerminal = fixture();
  falseTerminal.raw.runs.find((run) => run.treatment === "pro").completed = false;
  falseTerminal.rawBytes = Buffer.from(JSON.stringify(falseTerminal.raw));
  assert.throws(
    () => validateAndSanitize(falseTerminal),
    /completed flag and terminal status disagree/
  );
});

test("binds tool receipt status to a complete ordered lifecycle", () => {
  const metricByStatus = {
    succeeded: "tool_succeeded",
    failed: "tool_failed",
    cancelled: "tool_cancelled",
    denied: "tool_denied",
    superseded: "tool_superseded"
  };
  for (const [status, metric] of Object.entries(metricByStatus)) {
    const value = fixture();
    const fast = value.raw.runs.find((run) => run.treatment === "fast");
    fast.tool_receipts[0].status = status;
    fast.tool_receipts[0].finished_sequence = null;
    fast.metrics.tool_succeeded = 0;
    fast.metrics[metric] = 1;
    if (status !== "succeeded") {
      fast.tools_used = [];
      fast.verification.quality_passed = false;
      fast.verification.external_effect_passed = false;
    }
    value.rawBytes = Buffer.from(JSON.stringify(value.raw));
    assert.throws(
      () => validateAndSanitize(value),
      /terminal status lacks a complete lifecycle/
    );
  }

  const incomplete = fixture();
  const incompleteFast = incomplete.raw.runs.find((run) => run.treatment === "fast");
  incompleteFast.tool_receipts[0].status = "incomplete";
  incompleteFast.tool_receipts[0].started_sequence = null;
  incompleteFast.tool_receipts[0].finished_sequence = null;
  incompleteFast.tools_used = [];
  incompleteFast.metrics.tool_succeeded = 0;
  incompleteFast.metrics.tool_incomplete = 1;
  incompleteFast.verification.quality_passed = false;
  incompleteFast.verification.external_effect_passed = false;
  incomplete.rawBytes = Buffer.from(JSON.stringify(incomplete.raw));
  assert.throws(
    () => validateAndSanitize(incomplete),
    /incomplete call was never proposed or started/
  );

  const reversed = fixture();
  reversed.raw.runs.find(
    (run) => run.treatment === "fast"
  ).tool_receipts[0].started_sequence = 3;
  reversed.rawBytes = Buffer.from(JSON.stringify(reversed.raw));
  assert.throws(() => validateAndSanitize(reversed), /lifecycle sequence is invalid/);
});

test("binds product postcondition receipts to the frozen suite and complete evidence", () => {
  const value = fixture();
  const check = { path: "out/result.json", equals: { status: "ok" } };
  const immutable = { path: "spec.md", content: "frozen contract\n" };
  value.suite.cases[0].files = [immutable];
  value.suite.cases[0].verification.json_files = [check];
  value.suite.cases[0].verification.immutable_files = [immutable.path];
  value.suiteBytes = Buffer.from(JSON.stringify(value.suite));
  value.raw.suite_sha256 = hash(value.suiteBytes);
  const profileArtifacts = {
    auto: { ...value.raw.profile_artifacts.auto, path: null },
    pro: { ...value.raw.profile_artifacts.pro, path: null }
  };
  const plan = evaluationPlan(
    {
      suite: value.suite,
      suiteSha256: value.raw.suite_sha256,
      gitHead: value.raw.git_commit,
      replicates: 1,
      providerBinding: {
        provider_id: value.raw.provider_id,
        provider_endpoint: value.raw.provider_endpoint,
        configured_models: value.raw.configured_models
      },
      providerConfigSha256: value.raw.provider_config_sha256,
      profileArtifacts
    },
    {}
  );
  value.raw.execution_plan_sha256 = plan.sha256;
  const expected = JSON.stringify(check.equals);
  const subject = Buffer.concat([
    Buffer.from("cindx.agent-realworld-postcondition-subject.v1\0"),
    Buffer.from("json_file"),
    Buffer.from([0]),
    Buffer.from(check.path)
  ]);
  const immutableSubject = Buffer.concat([
    Buffer.from("cindx.agent-realworld-postcondition-subject.v1\0"),
    Buffer.from("immutable_fixture"),
    Buffer.from([0]),
    Buffer.from(immutable.path)
  ]);
  for (const run of value.raw.runs.filter((run) => run.treatment !== "direct")) {
    run.verification.passed_checks = 3;
    run.verification.total_checks = 3;
    run.verification.postcondition_receipts = [
      {
        kind: "json_file",
        subject_sha256: hash(subject),
        expected_sha256: hash(expected),
        observed_sha256: hash(expected),
        artifact_sha256: hash(expected),
        bytes: Buffer.byteLength(expected),
        passed: true
      },
      {
        kind: "immutable_fixture",
        subject_sha256: hash(immutableSubject),
        expected_sha256: hash(immutable.content),
        observed_sha256: hash(immutable.content),
        artifact_sha256: hash(immutable.content),
        bytes: Buffer.byteLength(immutable.content),
        passed: true
      }
    ];
  }
  value.rawBytes = Buffer.from(JSON.stringify(value.raw));
  validateAndSanitize(value);

  const missing = {
    ...value,
    raw: structuredClone(value.raw)
  };
  missing.raw.runs.find(
    (run) => run.treatment === "fast"
  ).verification.postcondition_receipts = [];
  missing.rawBytes = Buffer.from(JSON.stringify(missing.raw));
  assert.throws(
    () => validateAndSanitize(missing),
    /postcondition receipt count drifted from the frozen suite/
  );

  const drifted = {
    ...value,
    raw: structuredClone(value.raw)
  };
  drifted.raw.runs.find(
    (run) => run.treatment === "auto"
  ).verification.postcondition_receipts[0].expected_sha256 = hash("different");
  drifted.rawBytes = Buffer.from(JSON.stringify(drifted.raw));
  assert.throws(
    () => validateAndSanitize(drifted),
    /expectation drifted from the frozen suite/
  );

  const incompleteEvidence = {
    ...value,
    raw: structuredClone(value.raw)
  };
  const receipt = incompleteEvidence.raw.runs.find(
    (run) => run.treatment === "pro"
  ).verification.postcondition_receipts[0];
  receipt.observed_sha256 = null;
  receipt.artifact_sha256 = null;
  receipt.bytes = null;
  incompleteEvidence.rawBytes = Buffer.from(JSON.stringify(incompleteEvidence.raw));
  assert.throws(
    () => validateAndSanitize(incompleteEvidence),
    /passed result lacks observed artifact evidence/
  );
});

test("enforces the preregistered promotion gates without descriptive uplift", () => {
  const improved = fixture();
  const fast = improved.raw.runs.find((run) => run.treatment === "fast");
  fast.verification.quality_passed = false;
  fast.verification.answer_passed = false;
  improved.rawBytes = Buffer.from(JSON.stringify(improved.raw));
  const promoted = validateAndSanitize(improved);
  assert.equal(promoted.decision.uplift_status, "GO");
  assert.equal(promoted.decision.promotion_v1.minimum_improvement, "1/1");
  assert.equal(
    promoted.decision.promotion_v1.shared_gates.at_least_one_candidate_improves,
    true
  );

  const resourceRegression = fixture();
  const resourceFast = resourceRegression.raw.runs.find((run) => run.treatment === "fast");
  resourceFast.verification.quality_passed = false;
  resourceFast.verification.answer_passed = false;
  resourceRegression.raw.runs.find((run) => run.treatment === "auto").metrics.latency_ms = 31;
  resourceRegression.rawBytes = Buffer.from(JSON.stringify(resourceRegression.raw));
  const rejected = validateAndSanitize(resourceRegression);
  assert.equal(rejected.decision.uplift_status, "NO_GO");
  assert.equal(
    rejected.decision.promotion_v1.candidates.auto.gates.latency_within_limit,
    false
  );
});

test("fails closed on incomplete provider evidence and hashes provider identity", () => {
  const missingIdentity = fixture();
  missingIdentity.raw.provider_id = null;
  const directReceipt = missingIdentity.raw.runs.find(
    (run) => run.treatment === "direct"
  ).model_receipts[0];
  directReceipt.receipt_status = "provider_id_missing";
  directReceipt.provider_response_id_sha256 = null;
  const failed = missingIdentity.raw.runs.find((run) => run.treatment === "auto");
  failed.completed = false;
  failed.terminal_status = "failed";
  failed.evidence_error = "finished model response lacked a verifiable receipt";
  failed.model_receipts = [];
  failed.verification.quality_passed = false;
  failed.verification.answer_passed = false;
  missingIdentity.rawBytes = Buffer.from(JSON.stringify(missingIdentity.raw));

  const report = validateAndSanitize(missingIdentity);
  assert.deepEqual(report.evidence.provider_identity, {
    status: "no_provider_id",
    sha256: null
  });
  assert.equal(report.evidence.provider_evidence_complete, false);
  assert.equal(report.evidence.provider_evidence_incomplete_runs, 2);
  assert.equal(report.decision.uplift_status, "NO_GO");
  assert.equal(report.outcomes.non_completed_runs, 1);
  assert.equal(report.runs[0].model_receipts[0].receipt_status, "provider_id_missing");
  assert.equal(report.runs[0].model_receipts[0].provider_response_id_sha256, null);
  assert.equal(
    report.runs.find((run) => run.treatment === "auto").evidence_error_sha256,
    hash("finished model response lacked a verifiable receipt")
  );
});

test("rejects execution-order and strategy-artifact drift", () => {
  const executionDrift = fixture();
  executionDrift.raw.runs[0].execution_index = 2;
  executionDrift.rawBytes = Buffer.from(JSON.stringify(executionDrift.raw));
  assert.throws(() => validateAndSanitize(executionDrift), /not in execution order/);

  const strategyDrift = fixture();
  strategyDrift.raw.runs.find((run) => run.treatment === "auto").strategy_receipt.learned_artifact_sha256 =
    "c".repeat(64);
  strategyDrift.rawBytes = Buffer.from(JSON.stringify(strategyDrift.raw));
  assert.throws(() => validateAndSanitize(strategyDrift), /fresh-seed run must not claim/);
});

test("binds learned claims to the exact frozen profile receipt", () => {
  const frozen = fixture();
  const profileArtifacts = {
    auto: {
      mode: "frozen_profile",
      path: "/private/auto.json",
      file_sha256: "c".repeat(64),
      artifact_sha256: "d".repeat(64),
      candidate_sha256: "e".repeat(64),
      evolution_method: "gepa_reflective_paired"
    },
    pro: { ...frozen.raw.profile_artifacts.pro, path: null }
  };
  const plan = evaluationPlan(
    {
      suite: frozen.suite,
      suiteSha256: hash(frozen.suiteBytes),
      gitHead: frozen.raw.git_commit,
      replicates: 1,
      providerBinding: {
        provider_id: frozen.raw.provider_id,
        provider_endpoint: frozen.raw.provider_endpoint,
        configured_models: frozen.raw.configured_models
      },
      providerConfigSha256: frozen.raw.provider_config_sha256,
      profileArtifacts
    },
    {}
  );
  frozen.raw.execution_plan_sha256 = plan.sha256;
  frozen.raw.profile_artifacts = plan.profileArtifacts;
  const receipt = frozen.raw.runs.find((run) => run.treatment === "auto").strategy_receipt;
  receipt.profile_source = "evaluation_frozen_profile";
  receipt.profile_sha256 = "e".repeat(64);
  receipt.profile_generation = 1;
  receipt.parent_profile_ids = ["auto-stable"];
  receipt.learned_artifact_sha256 = "d".repeat(64);
  receipt.learned_method = "gepa_reflective_paired";
  receipt.stable_profile_id = "auto-stable";
  receipt.dataset_sha256 = "f".repeat(64);
  receipt.paired_evidence_sha256 = "1".repeat(64);
  receipt.promotion_gate_protocol = "paired-wilson-task-diversity-v1";
  frozen.rawBytes = Buffer.from(JSON.stringify(frozen.raw));

  const report = validateAndSanitize(frozen);
  assert.equal(report.decision.learned_profile_status, "FROZEN_ARTIFACT_EVALUATED");
  assert.equal(
    report.runs.find((run) => run.treatment === "auto").strategy_receipt
      .learned_artifact_sha256,
    "d".repeat(64)
  );
});

test("accepts legacy raw without setup failure and keeps error classification", () => {
  const legacy = fixture();
  for (const run of legacy.raw.runs) delete run.setup_failure;
  const failed = legacy.raw.runs[1];
  failed.completed = false;
  failed.terminal_status = "failed";
  failed.error = "request timed out";
  failed.verification.quality_passed = false;
  failed.verification.answer_passed = false;
  legacy.rawBytes = Buffer.from(JSON.stringify(legacy.raw));

  const report = validateAndSanitize(legacy);
  assert.equal(report.runs[1].setup_failure, null);
  assert.equal(report.runs[1].error_kind, "timeout");
  assert.equal(report.outcomes.by_treatment.fast.error_kind_counts.timeout, 1);
  assert.equal(report.runs[1].error, undefined);
});

test("classifies a frozen-deadline terminal status as a timeout", () => {
  const timedOut = fixture();
  const failed = timedOut.raw.runs[2];
  failed.completed = false;
  failed.terminal_status = "timed_out";
  failed.error = "evaluation process exceeded the frozen 600s deadline";
  failed.evidence_error = "run did not reach strategy/provider receipt collection";
  failed.strategy_receipt = null;
  failed.model_receipts = [];
  failed.metrics.model_responses = 0;
  failed.verification.quality_passed = false;
  failed.verification.answer_passed = false;
  failed.verification.external_effect_passed = false;
  timedOut.rawBytes = Buffer.from(JSON.stringify(timedOut.raw));

  const report = validateAndSanitize(timedOut);
  assert.equal(report.runs[2].error_kind, "timeout");
  assert.equal(report.outcomes.timeouts, 1);
  assert.equal(report.outcomes.by_treatment.auto.terminal_status_counts.timed_out, 1);
  assert.equal(report.evidence.strategy_evidence_incomplete_runs, 1);
  assert.equal(report.decision.uplift_status, "NO_GO");
});

test("rejects incomplete and tampered evidence", () => {
  const incomplete = fixture();
  incomplete.raw.runs.pop();
  incomplete.rawBytes = Buffer.from(JSON.stringify(incomplete.raw));
  assert.throws(() => validateAndSanitize(incomplete), /missing run/);

  const tampered = fixture();
  tampered.raw.runs[0].output = "changed";
  tampered.rawBytes = Buffer.from(JSON.stringify(tampered.raw));
  assert.throws(() => validateAndSanitize(tampered), /output hash mismatch/);
});

test("records safety failures without hiding the measured run", () => {
  const unsafe = fixture();
  unsafe.raw.runs[1].verification.safety_violations = 1;
  unsafe.rawBytes = Buffer.from(JSON.stringify(unsafe.raw));
  const report = validateAndSanitize(unsafe);
  assert.equal(report.decision.status, "SAFETY_FAILURE");
  assert.equal(report.decision.safety_violations, 1);
  assert.equal(report.decision.uplift_status, "NO_GO");
});

test("publishes infrastructure failures only as an invalid baseline", () => {
  const incomplete = fixture();
  const failed = incomplete.raw.runs[2];
  failed.completed = false;
  failed.terminal_status = "infrastructure_failed";
  failed.error = "memory seed failed with private provider detail";
  failed.setup_failure = { stage: "memory_seed", code: "transient", retryable: true };
  failed.metrics.setup_latency_ms = 37;
  failed.verification.quality_passed = false;
  failed.verification.answer_passed = false;
  failed.verification.external_effect_passed = null;
  failed.verification.failures = ["run did not reach verification"];
  incomplete.rawBytes = Buffer.from(JSON.stringify(incomplete.raw));

  const report = validateAndSanitize(incomplete);
  assert.equal(report.decision.status, "INVALID_BASELINE");
  assert.equal(report.evidence.complete_matrix, false);
  assert.equal(report.evidence.incomplete_runs, 1);
  assert.equal(report.outcomes.setup_failures, 1);
  assert.equal(report.decision.uplift_status, "NO_GO");
  assert.deepEqual(report.runs[2].setup_failure, {
    stage: "memory_seed",
    code: "transient",
    retryable: true
  });
  assert.equal(report.runs[2].error_kind, "transient");
  assert.equal(report.runs[2].metrics.setup_latency_ms, 37);
  assert.equal(report.runs[2].error, undefined);
  assert.match(report.decision.claim_boundary, /invalid for capability promotion/);
  const markdown = renderMarkdown(report);
  assert.match(markdown, /^# Cindx Agent Real-World Evaluation /);
  assert.match(markdown, /Missing or structurally unverifiable cells: 1/);
});

test("single-cell environment isolates evaluation data after provider loading", () => {
  const tempRoot = path.join(os.tmpdir(), "cindx-realworld-cell");
  const inheritedDataDir = path.join(os.tmpdir(), "cindx-production-provider-config");
  const env = evaluationEnvironment(
    { CINDX_DATA_DIR: inheritedDataDir },
    {
      gitHead: "a".repeat(40),
      suitePath: "/private/suite.json",
      replicates: 1,
      executionPlanSha256: "c".repeat(64),
      profileArtifacts: {
        auto: {
          mode: "frozen_profile",
          path: "/private/profile.json",
          file_sha256: "d".repeat(64),
          artifact_sha256: "e".repeat(64)
        }
      }
    },
    {
      replicate: 1,
      caseId: "case-a",
      treatment: "auto",
      executionIndex: 2,
      treatmentPosition: 2
    },
    "/private/raw.json",
    tempRoot
  );

  assert.equal(env.CINDX_DATA_DIR, inheritedDataDir);
  assert.equal(env.CINDX_AGENT_REALWORLD_DATA_DIR, path.join(tempRoot, ".cindx-eval-data"));
  assert.equal(env.CINDX_AGENT_REALWORLD_CASES, "case-a");
  assert.equal(env.CINDX_AGENT_REALWORLD_TREATMENTS, "auto");
  assert.equal(env.CINDX_AGENT_REALWORLD_EXECUTION_INDEX, "2");
  assert.equal(env.CINDX_AGENT_REALWORLD_TREATMENT_POSITION, "2");
  assert.equal(env.CINDX_AGENT_REALWORLD_PLAN_SHA256, "c".repeat(64));
  assert.equal(env.CINDX_AGENT_REALWORLD_PROFILE_PATH, "/private/profile.json");
  assert.equal(env.CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256, "e".repeat(64));
});

test("preflight rejects dirty or malformed provenance", () => {
  const suite = fixture().suite;
  assert.deepEqual(
    validatePreflight({ suite, gitHead: "a".repeat(40), status: "", requestedReplicates: "1" }),
    { replicates: 1 }
  );
  assert.throws(
    () => validatePreflight({ suite, gitHead: "a".repeat(40), status: " M file", requestedReplicates: "1" }),
    /worktree must be clean/
  );
  assert.throws(
    () =>
      validatePreflight({
        suite: { ...suite, execution_order: { ...suite.execution_order, protocol: "drift" } },
        gitHead: "a".repeat(40),
        status: "",
        requestedReplicates: "1"
      }),
    /execution-order contract mismatch/
  );
});

test("output boundary keeps raw evidence outside Git", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-realworld-output-"));
  const reports = path.join(root, "docs", "evaluations");
  fs.mkdirSync(reports, { recursive: true });
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-realworld-private-"));
  const valid = validateOutputPaths(root, {
    raw: path.join(outside, "raw.json"),
    sanitized: path.join(reports, "report.json"),
    markdown: path.join(reports, "report.md")
  });
  assert.equal(valid.raw, path.join(outside, "raw.json"));
  assert.throws(
    () =>
      validateOutputPaths(root, {
        raw: path.join(root, "raw.json"),
        sanitized: path.join(reports, "report.json"),
        markdown: path.join(reports, "report.md")
      }),
    /raw must remain outside/
  );
  fs.rmSync(root, { recursive: true, force: true });
  fs.rmSync(outside, { recursive: true, force: true });
});

test("runner requires explicit publish paths and execute opt-in", () => {
  const options = parseArguments([
      "--raw",
      "/private/raw.json",
      "--sanitized",
      "/private/report.json",
      "--markdown",
      "/private/report.md",
      "--auto-profile",
      "/private/auto.json",
      "--pro-profile",
      "/private/pro.json"
    ]);
  assert.equal(options.execute, false);
  assert.match(options.suite, /realworld-v5\.json$/);
  assert.equal(options.autoProfile, "/private/auto.json");
  assert.equal(options.proProfile, "/private/pro.json");
  assert.throws(() => parseArguments(["--execute"]), /--raw is required/);
});

test("validates private frozen profile bindings before execution", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-realworld-profile-root-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-realworld-profile-private-"));
  const snapshot = {
    schema: "cindx.prompt-profile-snapshot.v1",
    effort: "auto",
    genome: { id: "auto-candidate" },
    candidate_sha256: "a".repeat(64),
    stable_profile_id: "auto-stable",
    evolution_method: "gepa_reflective_paired",
    promotion_gate_protocol: "paired-wilson-task-diversity-v1"
  };
  const file = path.join(outside, "auto.json");
  fs.writeFileSync(file, JSON.stringify(snapshot));
  const binding = validateProfileArtifact(root, file, "auto");
  assert.equal(binding.mode, "frozen_profile");
  assert.equal(binding.path, fs.realpathSync(file));
  assert.equal(binding.file_sha256, hash(Buffer.from(JSON.stringify(snapshot))));
  assert.equal(binding.artifact_sha256, hash(Buffer.from(JSON.stringify(snapshot))));
  assert.equal(binding.candidate_sha256, "a".repeat(64));

  const inside = path.join(root, "profile.json");
  fs.writeFileSync(inside, JSON.stringify(snapshot));
  assert.throws(
    () => validateProfileArtifact(root, inside, "auto"),
    /must remain outside/
  );
  assert.throws(
    () => validateProfileArtifact(root, file, "pro"),
    /effort mismatch/
  );
  fs.rmSync(root, { recursive: true, force: true });
  fs.rmSync(outside, { recursive: true, force: true });
});

test("execution plan balances treatment positions and binds profile artifacts", () => {
  const suitePath = path.resolve("benchmarks/agent/realworld-v4.json");
  const suiteBytes = fs.readFileSync(suitePath);
  const suite = JSON.parse(suiteBytes);
  const fresh = {
    auto: {
      mode: "fresh_seed",
      path: null,
      file_sha256: null,
      artifact_sha256: null,
      candidate_sha256: null,
      evolution_method: null
    },
    pro: {
      mode: "fresh_seed",
      path: null,
      file_sha256: null,
      artifact_sha256: null,
      candidate_sha256: null,
      evolution_method: null
    }
  };
  const prepared = {
    suite,
    suiteSha256: hash(suiteBytes),
    gitHead: "b".repeat(40),
    replicates: suite.default_replicates,
    providerBinding: {
      provider_id: "fixture",
      provider_endpoint: "https://example.test/v1",
      configured_models: { default: "model-a" }
    },
    providerConfigSha256: "c".repeat(64),
    profileArtifacts: fresh
  };
  const plan = evaluationPlan(prepared, {});
  assert.equal(plan.entries.length, 72);
  for (const treatment of suite.treatments) {
    const counts = [1, 2, 3, 4].map(
      (position) =>
        plan.entries.filter(
          (entry) =>
            entry.treatment === treatment && entry.treatmentPosition === position
        ).length
    );
    assert.ok(Math.max(...counts) - Math.min(...counts) <= 1);
  }
  assert.deepEqual(plan.entries.slice(0, 8).map((entry) => entry.treatment), [
    "direct",
    "fast",
    "auto",
    "pro",
    "fast",
    "auto",
    "pro",
    "direct"
  ]);
  const partial = evaluationPlan(prepared, {
    cases: suite.cases[1].id,
    treatments: "auto"
  });
  assert.deepEqual(
    partial.entries.map((entry) => entry.executionIndex),
    plan.entries
      .filter(
        (entry) =>
          entry.caseId === suite.cases[1].id && entry.treatment === "auto"
      )
      .map((entry) => entry.executionIndex)
  );
  const frozen = structuredClone(prepared);
  frozen.profileArtifacts.auto = {
    mode: "frozen_profile",
    path: "/private/auto.json",
    file_sha256: "c".repeat(64),
    artifact_sha256: "e".repeat(64),
    candidate_sha256: "d".repeat(64),
    evolution_method: "gepa_reflective_paired"
  };
  assert.notEqual(evaluationPlan(frozen, {}).sha256, plan.sha256);
});

test("V5 freezes the authorized 72-cell grounded-direct plan", () => {
  const suitePath = path.resolve("benchmarks/agent/realworld-v5.json");
  const suiteBytes = fs.readFileSync(suitePath);
  const suite = JSON.parse(suiteBytes);
  const profileArtifacts = Object.fromEntries(
    ["auto", "pro"].map((effort) => [
      effort,
      {
        mode: "fresh_seed",
        path: null,
        file_sha256: null,
        artifact_sha256: null,
        candidate_sha256: null,
        evolution_method: null
      }
    ])
  );
  const prepared = {
    suite,
    suiteSha256: hash(suiteBytes),
    gitHead: "b".repeat(40),
    replicates: suite.default_replicates,
    providerBinding: {
      provider_id: "fixture",
      provider_endpoint: "https://example.test/v1",
      configured_models: { default: "model-a" }
    },
    providerConfigSha256: "c".repeat(64),
    profileArtifacts
  };
  const plan = evaluationPlan(prepared, {});
  assert.equal(validatePreflight({ suite, gitHead: prepared.gitHead, status: "" }).replicates, 3);
  assert.equal(plan.entries.length, 72);
  assert.deepEqual(plan.treatments, ["oracle_reference", "grounded_direct", "auto", "pro"]);
  for (const treatment of suite.treatments) {
    const positions = [1, 2, 3, 4].map(
      (position) =>
        plan.entries.filter(
          (entry) => entry.treatment === treatment && entry.treatmentPosition === position
        ).length
    );
    assert.ok(Math.max(...positions) - Math.min(...positions) <= 1);
  }
});

test("checkpoint resume rejects plan and profile drift", () => {
  const { suite, suiteBytes, raw } = fixture();
  const prepared = {
    suite,
    suiteSha256: hash(suiteBytes),
    gitHead: raw.git_commit,
    replicates: 1,
    providerBinding: {
      provider_id: raw.provider_id,
      provider_endpoint: raw.provider_endpoint,
      configured_models: raw.configured_models
    },
    providerConfigSha256: raw.provider_config_sha256,
    profileArtifacts: {
      auto: { ...raw.profile_artifacts.auto, path: null },
      pro: { ...raw.profile_artifacts.pro, path: null }
    }
  };
  const plan = evaluationPlan(prepared, {});
  validateCheckpoint(prepared, plan, raw);

  const drifted = structuredClone(raw);
  drifted.profile_artifacts.auto = {
    mode: "frozen_profile",
    file_sha256: "c".repeat(64),
    artifact_sha256: "e".repeat(64),
    candidate_sha256: "d".repeat(64),
    evolution_method: "gepa_reflective_paired"
  };
  assert.throws(() => validateCheckpoint(prepared, plan, drifted), /profile artifact mismatch/);

  const providerDrift = structuredClone(raw);
  providerDrift.provider_config_sha256 = "d".repeat(64);
  assert.throws(
    () => validateCheckpoint(prepared, plan, providerDrift),
    /provider configuration mismatch/
  );
});

test("temporary playwright resource is version-pinned and cleaned", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-realworld-playwright-"));
  const installed = path.join(root, "Applications", "Cindx.app");
  const source = path.join(
    installed,
    "Contents",
    "Resources",
    "sidecars",
    "node_modules",
    "playwright-core"
  );
  fs.mkdirSync(source, { recursive: true });
  fs.writeFileSync(
    path.join(source, "package.json"),
    JSON.stringify({ version: "1.61.1" })
  );
  const cleanup = preparePlaywrightResource(root, installed);
  const destination = path.join(root, "apps", "desktop", "node_modules", "playwright-core");
  assert.equal(fs.lstatSync(destination).isSymbolicLink(), true);
  cleanup();
  assert.equal(fs.existsSync(destination), false);
  fs.rmSync(root, { recursive: true, force: true });
});

test("checkpoint merge preserves matrix order and replaces an interrupted run", () => {
  const { raw } = fixture();
  const plan = {
    protocol: raw.execution_order_protocol,
    sha256: raw.execution_plan_sha256,
    providerConfigSha256: raw.provider_config_sha256,
    profileArtifacts: raw.profile_artifacts,
    cases: ["case-a"],
    treatments: ["direct", "fast", "auto", "pro"],
    entries: raw.runs.map((run) => ({
      replicate: run.replicate,
      caseId: run.case_id,
      treatment: run.treatment,
      executionIndex: run.execution_index,
      treatmentPosition: run.treatment_position
    }))
  };
  const interrupted = normalizeInterruptedRun(
    raw.runs[1],
    "timed_out",
    "evaluation process exceeded the frozen 600s deadline",
    600_000
  );
  const merged = mergeRunCheckpoint(raw, interrupted, plan, 1, raw.runs);
  assert.deepEqual(merged.runs.map(runKey), [
    "case-a/direct/r1",
    "case-a/fast/r1",
    "case-a/auto/r1",
    "case-a/pro/r1"
  ]);
  assert.equal(merged.runs[1].terminal_status, "timed_out");
  assert.equal(merged.runs[1].verification.external_effect_passed, false);
  assert.deepEqual(merged.runs[1].verification.postcondition_receipts, []);
  assert.equal(merged.runs[1].metrics.latency_ms, 600_000);

  const permissionRun = { ...raw.runs[1], category: "permission_safety" };
  const permissionTimeout = normalizeInterruptedRun(
    permissionRun,
    "timed_out",
    "deadline",
    600_000
  );
  assert.equal(permissionTimeout.verification.safety_violations, 1);
});
