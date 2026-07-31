import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const runner = path.join(repoRoot, "scripts", "run-paired-performance.mjs");

function git(root, args) {
  const result = spawnSync("git", args, { cwd: root, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
}

function fixtureManifest() {
  return {
    schema: "cindx.quality-gates.v1",
    id: "paired-performance-fixture",
    version: 1,
    priority_order: ["performance"],
    limitations: [],
    profiles: { "paired-performance": ["fixture-diagnostic"] },
    gates: [
      {
        id: "fixture-diagnostic",
        category: "performance",
        command: ["node", "emit-diagnostic.mjs"],
        required_output: ["cindx.fixture-diagnostic.v1"]
      }
    ]
  };
}

function fixturePolicy() {
  return {
    schema: "cindx.performance-policy.v1",
    id: "paired-performance-fixture",
    same_hardware_only: true,
    same_measurement_pair_only: true,
    same_profile_contract_only: true,
    required_profile: "paired-performance",
    workloads: [
      {
        schema: "cindx.fixture-diagnostic.v1",
        identity: ["items"],
        metrics: [
          {
            field: "p95_micros",
            max_regression_percent: 10,
            absolute_tolerance_micros: 0
          }
        ]
      }
    ]
  };
}

function createFixtureRepository(root, p95Micros) {
  fs.mkdirSync(path.join(root, "benchmarks", "system"), { recursive: true });
  fs.writeFileSync(
    path.join(root, "benchmarks", "system", "quality-gates-v1.json"),
    `${JSON.stringify(fixtureManifest(), null, 2)}\n`
  );
  fs.writeFileSync(
    path.join(root, "benchmarks", "system", "performance-policy-v1.json"),
    `${JSON.stringify(fixturePolicy(), null, 2)}\n`
  );
  fs.writeFileSync(
    path.join(root, "emit-diagnostic.mjs"),
    `console.log(JSON.stringify({ schema: "cindx.fixture-diagnostic.v1", items: 100, p95_micros: ${p95Micros} }));\n`
  );
  git(root, ["init", "-q"]);
  git(root, ["add", "."]);
  git(root, [
    "-c",
    "user.name=Cindx Test",
    "-c",
    "user.email=cindx-test@example.invalid",
    "commit",
    "-q",
    "-m",
    `fixture ${p95Micros}`
  ]);
}

function pairedRun(candidateMetric) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-paired-performance-"));
  const baselineRoot = path.join(directory, "baseline");
  const candidateRoot = path.join(directory, "candidate");
  const outputDir = path.join(directory, "reports");
  createFixtureRepository(baselineRoot, 100);
  createFixtureRepository(candidateRoot, candidateMetric);
  const result = spawnSync(
    process.execPath,
    [
      runner,
      "--baseline-root",
      baselineRoot,
      "--candidate-root",
      candidateRoot,
      "--output-dir",
      outputDir
    ],
    { cwd: repoRoot, encoding: "utf8" }
  );
  const reports = Object.fromEntries(
    ["baseline", "candidate", "comparison"].flatMap((name) => {
      const reportPath = path.join(outputDir, `${name}.json`);
      return fs.existsSync(reportPath)
        ? [[name, JSON.parse(fs.readFileSync(reportPath, "utf8"))]]
        : [];
    })
  );
  fs.rmSync(directory, { recursive: true, force: true });
  return { ...result, reports };
}

test("paired runner compares two commits on one measured environment", () => {
  const result = pairedRun(105);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.reports.comparison.passed, true);
  assert.equal(
    result.reports.baseline.performance_environment.machine_fingerprint,
    result.reports.candidate.performance_environment.machine_fingerprint
  );
  assert.ok(result.reports.baseline.performance_environment.pair_id);
  assert.equal(
    result.reports.baseline.performance_environment.pair_id,
    result.reports.candidate.performance_environment.pair_id
  );
});

test("paired runner fails when the candidate exceeds policy", () => {
  const result = pairedRun(150);
  assert.equal(result.status, 1);
  assert.equal(result.reports.comparison.passed, false);
  assert.equal(result.reports.comparison.comparisons[0].metrics[0].passed, false);
});

test("CI measures immutable base and head in one job and retains all reports", () => {
  const workflow = fs.readFileSync(
    path.join(repoRoot, ".github", "workflows", "ci.yml"),
    "utf8"
  );
  assert.match(workflow, /fetch-depth: 0/);
  assert.match(workflow, /id: performance-baseline/);
  assert.match(workflow, /ref: \$\{\{ steps\.performance-baseline\.outputs\.sha \}\}/);
  assert.match(workflow, /path: \.performance-baseline/);
  assert.match(
    workflow,
    /run: node scripts\/run-paired-performance\.mjs --baseline-root \.performance-baseline --candidate-root \. --output-dir target\/performance-regression/
  );
  for (const name of ["baseline", "candidate", "comparison"]) {
    assert.match(workflow, new RegExp(`target/performance-regression/${name}\\.json`));
  }
  const pairedRun = workflow.indexOf("Run paired same-machine performance regression");
  assert.ok(
    workflow.indexOf("working-directory: .performance-baseline/apps/desktop") < pairedRun
  );
  assert.ok(workflow.indexOf("working-directory: apps/desktop") < pairedRun);
});

test("paired policy contains only stable same-runner timing diagnostics", () => {
  const manifest = JSON.parse(
    fs.readFileSync(
      path.join(repoRoot, "benchmarks", "system", "quality-gates-v1.json"),
      "utf8"
    )
  );
  const policy = JSON.parse(
    fs.readFileSync(
      path.join(repoRoot, "benchmarks", "system", "performance-policy-v1.json"),
      "utf8"
    )
  );
  assert.deepEqual(manifest.profiles["paired-performance"], [
    "session-projection-scaling",
    "context-governor-scaling",
    "rag-search-scaling"
  ]);
  assert.equal(policy.required_profile, "paired-performance");
  assert.equal(policy.same_hardware_only, true);
  assert.equal(policy.same_measurement_pair_only, true);
  assert.equal(policy.same_profile_contract_only, true);
  assert.deepEqual(
    policy.workloads.map((workload) => workload.schema),
    [
      "cindx.session-projection-diagnostic.v1",
      "cindx.context-governor-diagnostic.v1",
      "cindx.rag-search-diagnostic.v1"
    ]
  );
});
