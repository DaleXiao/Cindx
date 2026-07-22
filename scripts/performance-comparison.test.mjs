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
    diagnostics: [
      {
        schema: "cindx.session-projection-diagnostic.v1",
        initial_events: 1000,
        delta_events_read: 1,
        warm_sample_count: 20,
        warm_p95_micros: 100,
        ...overrides.session
      },
      {
        schema: "cindx.context-governor-diagnostic.v1",
        history_messages: 8001,
        sample_count: 11,
        p95_micros: 70000,
        ...overrides.context
      },
      {
        schema: "cindx.rag-search-diagnostic.v1",
        chunks: 20000,
        dimensions: 64,
        sample_count: 11,
        semantic_p95_micros: 50000,
        literal_p95_micros: 175000,
        ...overrides.rag
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
