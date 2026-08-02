import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { validateAndSanitize } from "./agent-realworld-contract.mjs";
import {
  mergeRunCheckpoint,
  normalizeInterruptedRun,
  parseArguments,
  preparePlaywrightResource,
  runKey,
  validateOutputPaths,
  validatePreflight
} from "./run-agent-realworld.mjs";

function hash(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fixture() {
  const suite = {
    schema: "cindx.agent-realworld-suite.v1",
    id: "fixture",
    version: 1,
    description: "fixture",
    default_replicates: 1,
    per_run_timeout_seconds: 600,
    treatments: ["direct", "fast", "auto", "pro"],
    cases: [{ id: "case-a", category: "coding" }]
  };
  const suiteBytes = Buffer.from(JSON.stringify(suite));
  const output = "private answer";
  const runs = suite.treatments.map((treatment) => ({
    replicate: 1,
    case_id: "case-a",
    category: "coding",
    treatment,
    product_mechanism_exercised: treatment !== "direct",
    completed: true,
    terminal_status: "completed",
    configured_models: ["model-a"],
    tools_used: treatment === "direct" ? [] : ["file.read"],
    memory_records_after_seed: null,
    input_sha256: "a".repeat(64),
    output_sha256: hash(output),
    output,
    error: null,
    metrics: {
      latency_ms: treatment === "fast" ? 10 : 20,
      setup_latency_ms: 0,
      model_calls: 1,
      tool_calls: treatment === "direct" ? 0 : 1,
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
      external_effect_passed: treatment === "direct" ? null : true,
      passed_checks: 1,
      total_checks: 1,
      safety_violations: 0,
      failures: []
    }
  }));
  const raw = {
    schema: "cindx.agent-realworld-raw.v1",
    suite_id: suite.id,
    suite_version: suite.version,
    suite_description: suite.description,
    suite_sha256: hash(suiteBytes),
    generated_at_ms: 1,
    git_commit: "b".repeat(40),
    app_version: "0.1.82",
    provider_id: "fixture",
    provider_endpoint: "https://user:secret@example.test/v1?key=private",
    configured_models: { default: "model-a" },
    requested_replicates: 1,
    selected_cases: ["case-a"],
    selected_treatments: suite.treatments,
    runs
  };
  const rawBytes = Buffer.from(JSON.stringify(raw));
  return { suite, suiteBytes, raw, rawBytes };
}

test("validates the complete matrix and removes private output", () => {
  const report = validateAndSanitize(fixture());
  assert.equal(report.decision.status, "VALID_BASELINE");
  assert.equal(report.aggregates.fast.quality_pass_rate, 1);
  assert.equal(report.paired_against_fast.pro.quality_pass_delta, 0);
  assert.equal(report.runs[0].output, undefined);
  assert.equal(report.runs[0].error, undefined);
  assert.equal(report.evidence.provider_endpoint, "https://example.test/v1");
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
  assert.deepEqual(
    parseArguments([
      "--raw",
      "/private/raw.json",
      "--sanitized",
      "/private/report.json",
      "--markdown",
      "/private/report.md"
    ]).execute,
    false
  );
  assert.throws(() => parseArguments(["--execute"]), /--raw is required/);
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
    cases: ["case-a"],
    treatments: ["direct", "fast", "auto", "pro"],
    entries: raw.runs.map((run) => ({
      replicate: run.replicate,
      caseId: run.case_id,
      treatment: run.treatment
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
