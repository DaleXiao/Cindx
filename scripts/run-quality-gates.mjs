import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(
  repoRoot,
  "benchmarks",
  "system",
  "quality-gates-v1.json"
);
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const args = process.argv.slice(2);

function option(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}

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
      stdio: ["ignore", "pipe", "pipe"]
    });
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
      const errors =
        exitCode === 0
          ? [
              ...validateReport(gate),
              ...validateRequiredOutput(gate, requiredOutputObserved)
            ]
          : [`exit code ${exitCode}`];
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

const commit = spawnSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8"
}).stdout?.trim();
const startedAt = new Date().toISOString();
const results = [];
for (const gate of selectedGates) results.push(await runGate(gate));
const report = {
  schema: "cindx.quality-gate-report.v1",
  manifest_id: manifest.id,
  manifest_version: manifest.version,
  profile: profileName,
  commit: commit || null,
  started_at: startedAt,
  finished_at: new Date().toISOString(),
  passed: results.every((result) => result.passed),
  priority_order: manifest.priority_order,
  limitations: manifest.limitations,
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
