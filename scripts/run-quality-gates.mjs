import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const harnessRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);

function option(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}

const repoRoot = path.resolve(option("--repo-root", harnessRoot));
const manifestRoot = path.resolve(option("--manifest-root", repoRoot));
const manifestPath = path.join(
  manifestRoot,
  "benchmarks",
  "system",
  "quality-gates-v1.json"
);
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

const profileName = option("--profile", "quick");
const reportPath = path.resolve(
  repoRoot,
  option("--report", "target/quality-gate-report.json")
);
const selectedIds = manifest.profiles?.[profileName];
if (!Array.isArray(selectedIds)) {
  throw new Error(`Unknown quality gate profile: ${profileName}`);
}

const gateById = new Map(manifest.gates.map((gate) => [gate.id, gate]));
if (gateById.size !== manifest.gates.length) {
  throw new Error("Quality gate ids must be unique");
}
const selectedGates = selectedIds.map((id) => {
  const gate = gateById.get(id);
  if (!gate) throw new Error(`Profile ${profileName} references missing gate ${id}`);
  return gate;
});
const profileFingerprint = crypto
  .createHash("sha256")
  .update(JSON.stringify(selectedGates))
  .digest("hex");

const rustPath = path.join(
  os.homedir(),
  ".rustup",
  "toolchains",
  "stable-aarch64-apple-darwin",
  "bin"
);
const env = {
  ...process.env,
  PATH: [rustPath, path.join(os.homedir(), ".cargo", "bin"), process.env.PATH]
    .filter(Boolean)
    .join(path.delimiter)
};
const outputTailLimit = 16 * 1024;
const diagnosticLineLimit = 64 * 1024;
const diagnosticRecordLimit = 128;

function performanceEnvironment() {
  const cpus = os.cpus();
  const rustc = spawnSync("rustc", ["-Vv"], { env, encoding: "utf8" });
  const identity = {
    platform: process.platform,
    arch: process.arch,
    os_release: os.release(),
    hostname: os.hostname(),
    cpu_model: cpus[0]?.model ?? "unknown",
    logical_cpus: cpus.length,
    node_version: process.version,
    rustc_version: rustc.status === 0 ? rustc.stdout.trim() : null,
    runner_image:
      [process.env.ImageOS, process.env.ImageVersion].filter(Boolean).join("-") || null,
    build_profile: process.env.CINDX_PERFORMANCE_BUILD_PROFILE?.trim() || "test"
  };
  return {
    schema: "cindx.performance-environment.v1",
    machine_fingerprint: crypto
      .createHash("sha256")
      .update(JSON.stringify(identity))
      .digest("hex"),
    platform: identity.platform,
    arch: identity.arch,
    os_release: identity.os_release,
    cpu_model: identity.cpu_model,
    logical_cpus: identity.logical_cpus,
    node_version: identity.node_version,
    rustc_version: identity.rustc_version,
    runner_image: identity.runner_image,
    build_profile: identity.build_profile,
    pair_id: process.env.CINDX_PERFORMANCE_PAIR_ID?.trim() || null
  };
}

function appendTail(current, chunk) {
  const next = `${current}${chunk}`;
  return next.length > outputTailLimit ? next.slice(-outputTailLimit) : next;
}

function readPath(value, dottedPath) {
  return dottedPath.split(".").reduce((current, key) => current?.[key], value);
}

function validateReport(gate) {
  if (!gate.report) return [];
  const absolutePath = path.resolve(repoRoot, gate.report);
  if (!fs.existsSync(absolutePath)) return [`report missing: ${gate.report}`];
  const report = JSON.parse(fs.readFileSync(absolutePath, "utf8"));
  return (gate.assertions ?? []).flatMap((assertion) => {
    const actual = readPath(report, assertion.path);
    const expected = Object.hasOwn(assertion, "equals_path")
      ? readPath(report, assertion.equals_path)
      : assertion.equals;
    return Object.is(actual, expected)
      ? []
      : [`${assertion.path} expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`];
  });
}

function validateRequiredOutput(gate, observed) {
  return (gate.required_output ?? []).flatMap((expected) =>
    observed.has(expected) ? [] : [`required output missing: ${expected}`]
  );
}

function diagnosticRecords(output) {
  return output.split(/\r?\n/).flatMap((line) => {
    const objectStart = line.indexOf("{");
    if (objectStart < 0) return [];
    try {
      const value = JSON.parse(line.slice(objectStart));
      const schema = typeof value?.schema === "string" ? value.schema : "";
      return schema.startsWith("cindx.") &&
        (schema.includes("diagnostic") || schema.includes("scaling"))
        ? [value]
        : [];
    } catch {
      return [];
    }
  });
}

function createRequiredOutputObserver(expectedValues, observed) {
  const overlapLimit = Math.max(0, ...expectedValues.map((value) => value.length - 1));
  let carry = "";
  return (chunk) => {
    const output = `${carry}${chunk}`;
    for (const expected of expectedValues) {
      if (output.includes(expected)) observed.add(expected);
    }
    carry = overlapLimit > 0 ? output.slice(-overlapLimit) : "";
  };
}

function createDiagnosticCollector() {
  let carry = "";
  const records = [];
  const collect = (line) => {
    records.push(...diagnosticRecords(line));
    if (records.length > diagnosticRecordLimit) {
      records.splice(0, records.length - diagnosticRecordLimit);
    }
  };
  return {
    append(chunk) {
      const lines = `${carry}${chunk}`.split(/\r?\n/);
      carry = lines.pop() ?? "";
      for (const line of lines) collect(line);
      if (carry.length > diagnosticLineLimit) carry = carry.slice(-diagnosticLineLimit);
    },
    finish() {
      if (carry) collect(carry);
      carry = "";
      return records;
    }
  };
}

function runGate(gate) {
  return new Promise((resolve) => {
    const [executable, ...commandArgs] = gate.command;
    const startedAt = Date.now();
    let stdoutTail = "";
    let stderrTail = "";
    const requiredOutputObserved = new Set();
    const observeStdoutProof = createRequiredOutputObserver(
      gate.required_output ?? [],
      requiredOutputObserved
    );
    const observeStderrProof = createRequiredOutputObserver(
      gate.required_output ?? [],
      requiredOutputObserved
    );
    const stdoutDiagnostics = createDiagnosticCollector();
    const stderrDiagnostics = createDiagnosticCollector();
    let finalizedDiagnostics;
    const finishDiagnostics = () =>
      (finalizedDiagnostics ??= [
        ...stdoutDiagnostics.finish(),
        ...stderrDiagnostics.finish()
      ]);
    process.stdout.write(`\n[quality-gate] ${gate.id}\n`);
    const child = spawn(executable, commandArgs, {
      cwd: path.resolve(repoRoot, gate.cwd ?? "."),
      env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true
    });
    const timeoutMs = gate.timeout_ms ?? DEFAULT_GATE_TIMEOUT_MS;
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {
        child.kill("SIGKILL");
      }
    }, timeoutMs);
    child.stdout.on("data", (chunk) => {
      process.stdout.write(chunk);
      stdoutTail = appendTail(stdoutTail, chunk);
      observeStdoutProof(chunk);
      stdoutDiagnostics.append(chunk);
    });
    child.stderr.on("data", (chunk) => {
      process.stderr.write(chunk);
      stderrTail = appendTail(stderrTail, chunk);
      observeStderrProof(chunk);
      stderrDiagnostics.append(chunk);
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      resolve({
        id: gate.id,
        category: gate.category,
        passed: false,
        duration_ms: Date.now() - startedAt,
        exit_code: null,
        errors: [error.message],
        diagnostics: finishDiagnostics(),
        stdout_tail: stdoutTail,
        stderr_tail: stderrTail
      });
    });
    child.on("close", (exitCode) => {
      clearTimeout(timeout);
      const errors =
        exitCode === 0 && !timedOut
          ? [
              ...validateReport(gate),
              ...validateRequiredOutput(gate, requiredOutputObserved)
            ]
          : [timedOut ? `gate timed out after ${timeoutMs}ms` : `exit code ${exitCode}`];
      resolve({
        id: gate.id,
        category: gate.category,
        passed: exitCode === 0 && errors.length === 0,
        duration_ms: Date.now() - startedAt,
        exit_code: exitCode,
        errors,
        diagnostics: finishDiagnostics(),
        stdout_tail: stdoutTail,
        stderr_tail: stderrTail
      });
    });
  });
}

function gitCommit(root) {
  return spawnSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8"
  }).stdout?.trim();
}

function gitOutput(root, args) {
  return spawnSync("git", args, { cwd: root, encoding: "utf8" }).stdout ?? "";
}

// The report binds the exact source state it measured: HEAD, whether the
// worktree was dirty, and a digest of the tracked index. Readers can reject
// reports whose identity does not match the revision under review.
function sourceIdentity(root) {
  const status = gitOutput(root, ["status", "--porcelain"]);
  const tracked = gitOutput(root, ["ls-files", "-s"]);
  return {
    worktree_dirty: status.trim().length > 0,
    tracked_tree_digest: crypto.createHash("sha256").update(tracked).digest("hex")
  };
}

const DEFAULT_GATE_TIMEOUT_MS = 20 * 60 * 1000;

const commit = gitCommit(repoRoot);
const manifestCommit = gitCommit(manifestRoot);
const identity = sourceIdentity(repoRoot);
const startedAt = new Date().toISOString();
// A stale report must never satisfy a fresh run's report-backed gates.
if (fs.existsSync(reportPath)) fs.rmSync(reportPath);
const results = [];
for (const gate of selectedGates) results.push(await runGate(gate));
const report = {
  schema: "cindx.quality-gate-report.v1",
  manifest_id: manifest.id,
  manifest_version: manifest.version,
  manifest_commit: manifestCommit || null,
  profile: profileName,
  profile_fingerprint: profileFingerprint,
  commit: commit || null,
  worktree_dirty: identity.worktree_dirty,
  tracked_tree_digest: identity.tracked_tree_digest,
  run_nonce: crypto.randomUUID(),
  started_at: startedAt,
  finished_at: new Date().toISOString(),
  passed: results.every((result) => result.passed),
  priority_order: manifest.priority_order,
  limitations: manifest.limitations,
  performance_environment: performanceEnvironment(),
  diagnostics: results.flatMap((result) =>
    result.diagnostics.map((diagnostic) => ({ gate_id: result.id, ...diagnostic }))
  ),
  results
};
fs.mkdirSync(path.dirname(reportPath), { recursive: true });
fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
process.stdout.write(`\nQuality gate report: ${path.relative(repoRoot, reportPath)}\n`);
process.stdout.write(report.passed ? "Quality gates passed\n" : "Quality gates failed\n");
if (!report.passed) process.exitCode = 1;
