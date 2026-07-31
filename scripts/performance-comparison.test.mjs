import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const comparator = path.join(repoRoot, "scripts", "compare-performance-reports.mjs");
const policy = path.join(
  repoRoot,
  "benchmarks",
  "system",
  "performance-policy-v1.json"
);

function report(overrides = {}) {
  return {
    schema: "cindx.quality-gate-report.v1",
    passed: overrides.passed ?? true,
    profile: overrides.profile ?? "paired-performance",
    profile_fingerprint: overrides.profileFingerprint ?? "performance-profile-v1",
    manifest_commit: overrides.manifestCommit ?? "manifest-commit",
    performance_environment:
      overrides.environment === null
        ? undefined
        : {
            schema: "cindx.performance-environment.v1",
            machine_fingerprint: "same-machine",
            pair_id: "same-pair",
            ...overrides.environment
          },
    diagnostics: [
      {
        schema: "cindx.session-projection-diagnostic.v1",
        initial_events: 1000,
        unrelated_events: 4950,
        delta_events_read: 1,
        warm_sample_count: 20,
        warm_p95_micros: 100,
        ...overrides.session
      },
      {
        schema: "cindx.context-governor-diagnostic.v1",
        history_messages: 8001,
        history_payload_bytes: 1177836,
        sample_count: 11,
        p95_micros: 70000,
        ...overrides.context
      },
      {
        schema: "cindx.rag-search-diagnostic.v1",
        chunks: 20000,
        dimensions: 64,
        estimated_payload_bytes: 7215560,
        sample_count: 11,
        semantic_p95_micros: 50000,
        literal_p95_micros: 175000,
        ...overrides.rag
      },
      {
        schema: "cindx.conductor-health-diagnostic.v1",
        models: 6,
        max_health_keys: 32,
        max_observations_per_key: 32,
        warmup_rounds: 20,
        sample_count: 101,
        hedge: false,
        cold_path_p95_micros: 1,
        warm_evidence_p95_micros: 10,
        ...overrides.health
      }
    ]
  };
}

function compare(candidate) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-performance-policy-"));
  const baselinePath = path.join(directory, "baseline.json");
  const candidatePath = path.join(directory, "candidate.json");
  fs.writeFileSync(baselinePath, JSON.stringify(report()));
  fs.writeFileSync(candidatePath, JSON.stringify(candidate));
  const result = spawnSync(
    process.execPath,
    [comparator, "--baseline", baselinePath, "--candidate", candidatePath, "--policy", policy],
    { cwd: repoRoot, encoding: "utf8" }
  );
  fs.rmSync(directory, { recursive: true, force: true });
  return { ...result, output: JSON.parse(result.stdout) };
}

test("same-machine policy accepts measurements inside per-metric tolerance", () => {
  const result = compare(
    report({
      session: { warm_p95_micros: 340 },
      context: { p95_micros: 75000 },
      rag: { semantic_p95_micros: 53500, literal_p95_micros: 187000 }
    })
  );
  assert.equal(result.status, 0);
  assert.equal(result.output.passed, true);
});

test("same-machine policy rejects a latency regression", () => {
  const result = compare(report({ context: { p95_micros: 78000 } }));
  assert.equal(result.status, 1);
  assert.equal(result.output.passed, false);
  const context = result.output.comparisons.find(
    (entry) => entry.schema === "cindx.context-governor-diagnostic.v1"
  );
  assert.equal(context.metrics[0].passed, false);
});

test("same-machine policy rejects a different workload", () => {
  const result = compare(report({ rag: { chunks: 10000 } }));
  assert.equal(result.status, 1);
  assert.match(result.output.comparisons[2].errors[0], /chunks differs/);
});

test("same-machine policy rejects a different input payload", () => {
  const result = compare(report({ context: { history_payload_bytes: 900000 } }));
  assert.equal(result.status, 1);
  assert.match(result.output.comparisons[1].errors.join("\n"), /history_payload_bytes differs/);
});

test("same-machine policy rejects missing environment evidence", () => {
  const result = compare(report({ environment: null }));
  assert.equal(result.status, 1);
  assert.equal(result.output.compatibility.passed, false);
  assert.match(result.output.compatibility.errors.join("\n"), /environment|fingerprint|pair id/);
});

test("same-machine policy rejects a different runner", () => {
  const result = compare(
    report({ environment: { machine_fingerprint: "different-machine" } })
  );
  assert.equal(result.status, 1);
  assert.match(result.output.compatibility.errors.join("\n"), /machine fingerprints differ/);
});

test("same-machine policy rejects a different measurement pair", () => {
  const result = compare(report({ environment: { pair_id: "different-pair" } }));
  assert.equal(result.status, 1);
  assert.match(result.output.compatibility.errors.join("\n"), /pair ids differ/);
});

test("same-machine policy rejects a different performance profile contract", () => {
  const result = compare(report({ profileFingerprint: "different-profile" }));
  assert.equal(result.status, 1);
  assert.match(result.output.compatibility.errors.join("\n"), /profile fingerprints differ/);
});

test("same-machine policy rejects a non-paired performance report", () => {
  const result = compare(report({ profile: "shipping-performance" }));
  assert.equal(result.status, 1);
  assert.match(
    result.output.compatibility.errors.join("\n"),
    /candidate profile must be paired-performance/
  );
});

test("same-machine policy rejects a different manifest commit", () => {
  const result = compare(report({ manifestCommit: "different-manifest" }));
  assert.equal(result.status, 1);
  assert.match(result.output.compatibility.errors.join("\n"), /manifest commits differ/);
});

test("same-machine policy rejects a failed source report", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-performance-policy-"));
  const baselinePath = path.join(directory, "baseline.json");
  const candidatePath = path.join(directory, "candidate.json");
  fs.writeFileSync(baselinePath, JSON.stringify(report()));
  fs.writeFileSync(candidatePath, JSON.stringify(report({ passed: false })));
  const result = spawnSync(
    process.execPath,
    [comparator, "--baseline", baselinePath, "--candidate", candidatePath, "--policy", policy],
    { cwd: repoRoot, encoding: "utf8" }
  );
  fs.rmSync(directory, { recursive: true, force: true });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /did not pass its source quality gates/);
});

test("same-machine policy rejects duplicate diagnostic schemas", () => {
  const candidate = report();
  candidate.diagnostics.push({ ...candidate.diagnostics[0] });
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-performance-policy-"));
  const baselinePath = path.join(directory, "baseline.json");
  const candidatePath = path.join(directory, "candidate.json");
  fs.writeFileSync(baselinePath, JSON.stringify(report()));
  fs.writeFileSync(candidatePath, JSON.stringify(candidate));
  const result = spawnSync(
    process.execPath,
    [comparator, "--baseline", baselinePath, "--candidate", candidatePath, "--policy", policy],
    { cwd: repoRoot, encoding: "utf8" }
  );
  fs.rmSync(directory, { recursive: true, force: true });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /duplicate diagnostic schema/);
});

test("unversioned diagnostic comparison keeps the legacy report shape", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-performance-policy-"));
  const baselinePath = path.join(directory, "baseline.json");
  const candidatePath = path.join(directory, "candidate.json");
  fs.writeFileSync(baselinePath, JSON.stringify({ diagnostics: report().diagnostics }));
  fs.writeFileSync(candidatePath, JSON.stringify({ diagnostics: report().diagnostics }));
  const result = spawnSync(
    process.execPath,
    [comparator, "--baseline", baselinePath, "--candidate", candidatePath],
    { cwd: repoRoot, encoding: "utf8" }
  );
  fs.rmSync(directory, { recursive: true, force: true });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).passed, true);
});
