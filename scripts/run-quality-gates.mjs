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

function diagnosticRecords(output) {
  return output.split(/\r?\n/).flatMap((line) => {
    const objectStart = line.indexOf("{");
    if (objectStart < 0) return [];
    try {
      const value = JSON.parse(line.slice(objectStart));
      return typeof value?.schema === "string" && value.schema.includes("diagnostic")
        ? [value]
        : [];
    } catch {
      return [];
    }
  });
}

function runGate(gate) {
  return new Promise((resolve) => {
    const [executable, ...commandArgs] = gate.command;
    const startedAt = Date.now();
    let stdoutTail = "";
    let stderrTail = "";
    process.stdout.write(`\n[quality-gate] ${gate.id}\n`);
    const child = spawn(executable, commandArgs, {
      cwd: path.resolve(repoRoot, gate.cwd ?? "."),
      env,
      stdio: ["ignore", "pipe", "pipe"]
    });
    child.stdout.on("data", (chunk) => {
      process.stdout.write(chunk);
      stdoutTail = appendTail(stdoutTail, chunk);
    });
    child.stderr.on("data", (chunk) => {
      process.stderr.write(chunk);
      stderrTail = appendTail(stderrTail, chunk);
    });
    child.on("error", (error) => {
      resolve({
        id: gate.id,
        category: gate.category,
        passed: false,
        duration_ms: Date.now() - startedAt,
        exit_code: null,
        errors: [error.message],
        diagnostics: diagnosticRecords(stdoutTail),
        stdout_tail: stdoutTail,
        stderr_tail: stderrTail
      });
    });
    child.on("close", (exitCode) => {
      const errors = exitCode === 0 ? validateReport(gate) : [`exit code ${exitCode}`];
      resolve({
        id: gate.id,
        category: gate.category,
        passed: exitCode === 0 && errors.length === 0,
        duration_ms: Date.now() - startedAt,
        exit_code: exitCode,
        errors,
        diagnostics: diagnosticRecords(stdoutTail),
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
