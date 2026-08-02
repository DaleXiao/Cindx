import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const defaultSuite = path.join(repositoryRoot, "benchmarks", "agent", "realworld-v2.json");

function requireFact(condition, message) {
  if (!condition) throw new Error(message);
}

function requiredValue(argv, index, flag) {
  const value = argv[index + 1];
  requireFact(value && !value.startsWith("--"), `${flag} requires a value`);
  return value;
}

export function parseArguments(argv) {
  const options = { execute: false, suite: defaultSuite };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--execute") {
      options.execute = true;
      continue;
    }
    if (argument === "--help" || argument === "-h") return { help: true };
    const key = {
      "--suite": "suite",
      "--raw": "raw",
      "--sanitized": "sanitized",
      "--markdown": "markdown",
      "--replicates": "replicates",
      "--cases": "cases",
      "--treatments": "treatments"
    }[argument];
    requireFact(key, `unknown argument ${argument}`);
    requireFact(options[key] === undefined || key === "suite", `${argument} may be supplied once`);
    options[key] = requiredValue(argv, index, argument);
    index += 1;
  }
  for (const key of ["raw", "sanitized", "markdown"]) {
    requireFact(options[key], `--${key} is required`);
  }
  return options;
}

function isWithin(directory, candidate) {
  const relative = path.relative(path.resolve(directory), path.resolve(candidate));
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== "..");
}

export function validateOutputPaths(root, outputs) {
  const resolved = Object.fromEntries(
    Object.entries(outputs).map(([key, value]) => [key, path.resolve(value)])
  );
  requireFact(!isWithin(root, resolved.raw), "--raw must remain outside the Git checkout");
  const reports = path.join(root, "docs", "evaluations");
  for (const key of ["sanitized", "markdown"]) {
    if (isWithin(root, resolved[key])) {
      requireFact(isWithin(reports, resolved[key]), `--${key} may be tracked only under docs/evaluations`);
    }
  }
  requireFact(new Set(Object.values(resolved)).size === 3, "output paths must be distinct");
  return resolved;
}

function checked(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    ...options
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = options.stdio === "inherit" ? "" : `: ${(result.stderr || "").trim()}`;
    throw new Error(`${command} failed with status ${result.status}${detail}`);
  }
  return (result.stdout || "").trim();
}

function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function parseProviderConfig(file) {
  requireFact(fs.existsSync(file), "Cindx provider configuration is missing");
  const values = new Map();
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    const separator = line.indexOf("=");
    if (separator < 1) continue;
    values.set(line.slice(0, separator), line.slice(separator + 1));
  }
  for (const key of ["provider_id", "base_url", "api_key", "model"]) {
    requireFact(values.get(key)?.trim(), `provider configuration is missing ${key}`);
  }
}

function expectedPlaywrightVersion() {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(repositoryRoot, "apps", "desktop", "package.json"), "utf8")
  );
  return manifest.dependencies?.["playwright-core"];
}

export function preparePlaywrightResource(root, installedApp = "/Applications/Cindx.app") {
  const destination = path.join(root, "apps", "desktop", "node_modules", "playwright-core");
  const destinationManifest = path.join(destination, "package.json");
  const expectedVersion = expectedPlaywrightVersion();
  if (fs.existsSync(destinationManifest)) {
    const actual = JSON.parse(fs.readFileSync(destinationManifest, "utf8")).version;
    requireFact(actual === expectedVersion, `workspace playwright-core ${actual} != ${expectedVersion}`);
    return () => {};
  }
  if (fs.existsSync(destination)) fs.rmSync(destination, { recursive: true, force: true });
  const source = path.join(
    installedApp,
    "Contents",
    "Resources",
    "sidecars",
    "node_modules",
    "playwright-core"
  );
  const sourceManifest = path.join(source, "package.json");
  requireFact(fs.existsSync(sourceManifest), "installed Cindx playwright-core is unavailable");
  const installedVersion = JSON.parse(fs.readFileSync(sourceManifest, "utf8")).version;
  requireFact(
    installedVersion === expectedVersion,
    `installed playwright-core ${installedVersion} != frozen ${expectedVersion}`
  );
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.symlinkSync(source, destination, "dir");
  return () => {
    fs.rmSync(destination, { force: true });
    try {
      fs.rmdirSync(path.dirname(destination));
    } catch {
      // A pre-existing node_modules directory is left intact.
    }
  };
}

export function validatePreflight({ suite, gitHead, status, requestedReplicates }) {
  requireFact(suite.schema === "cindx.agent-realworld-suite.v1", "suite schema mismatch");
  requireFact(
    Number.isInteger(suite.per_run_timeout_seconds) &&
      suite.per_run_timeout_seconds >= 60 &&
      suite.per_run_timeout_seconds <= 3600,
    "suite per-run timeout must be between 60 and 3600 seconds"
  );
  requireFact(/^[0-9a-f]{40}$/.test(gitHead), "Git HEAD must be a full lowercase SHA");
  requireFact(!status.trim(), "worktree must be clean before provider-backed evaluation");
  const replicates = requestedReplicates
    ? Number.parseInt(requestedReplicates, 10)
    : suite.default_replicates;
  requireFact(Number.isInteger(replicates) && replicates > 0, "replicates must be positive");
  return { replicates };
}

function preflight(options) {
  const suitePath = path.resolve(options.suite);
  requireFact(fs.statSync(suitePath).isFile(), "--suite must reference a file");
  const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
  const gitHead = checked("git", ["rev-parse", "HEAD"]);
  const status = checked("git", ["status", "--porcelain"]);
  const facts = validatePreflight({
    suite,
    gitHead,
    status,
    requestedReplicates: options.replicates
  });
  parseProviderConfig(
    path.join(process.env.HOME, "Library", "Application Support", "Cindx", "provider.conf")
  );
  return {
    ...facts,
    suite,
    suitePath,
    suiteSha256: sha256File(suitePath),
    gitHead,
    outputs: validateOutputPaths(repositoryRoot, {
      raw: options.raw,
      sanitized: options.sanitized,
      markdown: options.markdown
    })
  };
}

function cargoBuildInvocation() {
  return {
    command: process.env.CARGO || "cargo",
    args: [
      "build",
      "--locked",
      "--manifest-path",
      path.join(repositoryRoot, "apps", "desktop", "src-tauri", "Cargo.toml"),
      "--features",
      "realworld-eval",
      "--bin",
      "cindx-agent-realworld-eval"
    ]
  };
}

function evaluationBinary() {
  const targetRoot = process.env.CARGO_TARGET_DIR
    ? path.resolve(process.env.CARGO_TARGET_DIR)
    : path.join(repositoryRoot, "apps", "desktop", "src-tauri", "target");
  return path.join(targetRoot, "debug", "cindx-agent-realworld-eval");
}

function analyzerInvocation(prepared) {
  return {
    command: process.execPath,
    args: [
      path.join(repositoryRoot, "scripts", "agent-realworld-contract.mjs"),
      "--suite",
      prepared.suitePath,
      "--raw",
      prepared.outputs.raw,
      "--sanitized",
      prepared.outputs.sanitized,
      "--markdown",
      prepared.outputs.markdown
    ]
  };
}

function evaluationEnvironment(base, prepared, entry, output, tempRoot) {
  const playwrightModules = path.join(repositoryRoot, "apps", "desktop", "node_modules");
  const nodePath = base.NODE_PATH
    ? `${playwrightModules}${path.delimiter}${base.NODE_PATH}`
    : playwrightModules;
  return {
    ...base,
    NODE_PATH: nodePath,
    CINDX_EVAL_GIT_COMMIT: prepared.gitHead,
    CINDX_AGENT_REALWORLD_SUITE: prepared.suitePath,
    CINDX_AGENT_REALWORLD_OUTPUT: output,
    CINDX_AGENT_REALWORLD_REPLICATES: String(prepared.replicates),
    CINDX_AGENT_REALWORLD_REPLICATE_INDEX: String(entry.replicate),
    CINDX_AGENT_REALWORLD_CASES: entry.caseId,
    CINDX_AGENT_REALWORLD_TREATMENTS: entry.treatment,
    CINDX_AGENT_REALWORLD_TEMP_ROOT: tempRoot
  };
}

function selectedInSuite(values, requested, label) {
  if (!requested) return [...values];
  const requestedValues = new Set(requested.split(",").map((value) => value.trim()).filter(Boolean));
  const selected = values.filter((value) => requestedValues.has(value));
  requireFact(selected.length === requestedValues.size && selected.length > 0, `${label} selection is invalid`);
  return selected;
}

function evaluationPlan(prepared, options) {
  const cases = selectedInSuite(
    prepared.suite.cases.map((testCase) => testCase.id),
    options.cases,
    "case"
  );
  const treatments = selectedInSuite(prepared.suite.treatments, options.treatments, "treatment");
  const entries = [];
  for (let replicate = 1; replicate <= prepared.replicates; replicate += 1) {
    for (const caseId of cases) {
      for (const treatment of treatments) entries.push({ replicate, caseId, treatment });
    }
  }
  return { cases, treatments, entries };
}

export function runKey(run) {
  return `${run.case_id}/${run.treatment}/r${run.replicate}`;
}

export function normalizeInterruptedRun(run, terminalStatus, error, latencyMs) {
  const productRun = run.treatment !== "direct";
  const safetyUnverified = productRun && run.category === "permission_safety";
  return {
    ...run,
    completed: false,
    terminal_status: terminalStatus,
    output: "",
    output_sha256: crypto.createHash("sha256").update("").digest("hex"),
    error,
    metrics: { ...run.metrics, latency_ms: latencyMs },
    verification: {
      ...run.verification,
      quality_passed: false,
      answer_passed: false,
      external_effect_passed: productRun ? false : null,
      passed_checks: 0,
      total_checks: Math.max(1, run.verification?.total_checks || 0),
      safety_violations: safetyUnverified ? 1 : 0,
      failures: [
        safetyUnverified
          ? "permission safety could not be verified before the run stopped"
          : "run did not reach verification"
      ]
    }
  };
}

function writePrivateJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const temporary = `${file}.${process.pid}.${crypto.randomBytes(6).toString("hex")}.tmp`;
  fs.writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  fs.renameSync(temporary, file);
  fs.chmodSync(file, 0o600);
}

function validateCheckpoint(prepared, plan, raw) {
  requireFact(raw?.schema === "cindx.agent-realworld-raw.v1", "checkpoint schema mismatch");
  requireFact(raw.git_commit === prepared.gitHead, "checkpoint Git commit mismatch");
  requireFact(raw.suite_sha256 === prepared.suiteSha256, "checkpoint suite hash mismatch");
  requireFact(raw.requested_replicates === prepared.replicates, "checkpoint replicate count mismatch");
  requireFact(JSON.stringify(raw.selected_cases) === JSON.stringify(plan.cases), "checkpoint case selection mismatch");
  requireFact(
    JSON.stringify(raw.selected_treatments) === JSON.stringify(plan.treatments),
    "checkpoint treatment selection mismatch"
  );
  const expected = new Set(plan.entries.map((entry) => `${entry.caseId}/${entry.treatment}/r${entry.replicate}`));
  const seen = new Set();
  for (const run of raw.runs || []) {
    const key = runKey(run);
    requireFact(expected.has(key), `checkpoint contains unexpected run ${key}`);
    requireFact(!seen.has(key), `checkpoint contains duplicate run ${key}`);
    seen.add(key);
  }
}

function checkpointRuns(prepared, plan) {
  if (!fs.existsSync(prepared.outputs.raw)) return new Map();
  const raw = JSON.parse(fs.readFileSync(prepared.outputs.raw, "utf8"));
  validateCheckpoint(prepared, plan, raw);
  return new Map(
    raw.runs
      .filter((run) => run.terminal_status !== "running")
      .map((run) => [runKey(run), run])
  );
}

export function mergeRunCheckpoint(baseReport, run, plan, replicates, existingRuns = []) {
  const runs = new Map(existingRuns.map((item) => [runKey(item), item]));
  runs.set(runKey(run), run);
  return {
    ...baseReport,
    generated_at_ms: Date.now(),
    requested_replicates: replicates,
    selected_cases: [...plan.cases],
    selected_treatments: [...plan.treatments],
    runs: plan.entries.map((entry) => runs.get(`${entry.caseId}/${entry.treatment}/r${entry.replicate}`)).filter(Boolean)
  };
}

function terminateRunProcesses(tempRoot) {
  if (process.platform !== "darwin") return;
  spawnSync("pkill", ["-TERM", "-f", tempRoot], { stdio: "ignore" });
  spawnSync("pkill", ["-KILL", "-f", tempRoot], { stdio: "ignore" });
}

function runSingleEvaluation(binary, prepared, entry) {
  const runRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-agent-realworld-run-"));
  const workspaceRoot = path.join(runRoot, "workspace");
  const partRaw = path.join(runRoot, "raw.json");
  const timeoutMs = prepared.suite.per_run_timeout_seconds * 1000;
  let result;
  try {
    result = spawnSync(binary, [], {
      cwd: repositoryRoot,
      env: evaluationEnvironment(process.env, prepared, entry, partRaw, workspaceRoot),
      stdio: "inherit",
      timeout: timeoutMs,
      killSignal: "SIGTERM"
    });
    requireFact(fs.existsSync(partRaw), `run ${entry.caseId}/${entry.treatment}/r${entry.replicate} produced no checkpoint`);
    const report = JSON.parse(fs.readFileSync(partRaw, "utf8"));
    requireFact(report.git_commit === prepared.gitHead, "run checkpoint Git commit mismatch");
    requireFact(report.suite_sha256 === prepared.suiteSha256, "run checkpoint suite hash mismatch");
    requireFact(report.runs?.length === 1, "single-run process produced an invalid run count");
    let run = report.runs[0];
    const timedOut = result.error?.code === "ETIMEDOUT";
    if (timedOut) {
      run = normalizeInterruptedRun(
        run,
        "timed_out",
        `evaluation process exceeded the frozen ${prepared.suite.per_run_timeout_seconds}s deadline`,
        timeoutMs
      );
    } else if (result.error || result.status !== 0 || run.terminal_status === "running") {
      run = normalizeInterruptedRun(
        run,
        "infrastructure_failed",
        result.error?.message || `evaluation process exited with status ${result.status}`,
        run.metrics?.latency_ms || 0
      );
    }
    return { report, run };
  } finally {
    terminateRunProcesses(workspaceRoot);
    fs.rmSync(runRoot, { recursive: true, force: true });
  }
}

function validatePostflight(prepared) {
  requireFact(checked("git", ["rev-parse", "HEAD"]) === prepared.gitHead, "Git HEAD changed during evaluation");
  requireFact(!checked("git", ["status", "--porcelain"]).trim(), "worktree changed during evaluation");
  const raw = JSON.parse(fs.readFileSync(prepared.outputs.raw, "utf8"));
  requireFact(raw.git_commit === prepared.gitHead, "raw evidence is not bound to evaluated HEAD");
  requireFact(raw.suite_sha256 === prepared.suiteSha256, "raw evidence suite hash drifted");
}

function usage() {
  return [
    "Usage: node scripts/run-agent-realworld.mjs [--execute] \\",
    "  --raw PATH --sanitized PATH --markdown PATH [--suite PATH] [--replicates N] \\",
    "  [--cases id,id] [--treatments direct,fast,auto,pro]",
    "",
    "Without --execute, only Git, provider, suite, and output-boundary preflight runs.",
    "A publishable report requires the suite's full case/treatment matrix and replicate count."
  ].join("\n");
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArguments(argv);
  if (options.help) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const prepared = preflight(options);
  process.stdout.write(
    `Agent real-world preflight passed at ${prepared.gitHead.slice(0, 12)}; suite ${prepared.suiteSha256.slice(0, 12)}.\n`
  );
  if (!options.execute) {
    process.stdout.write("No provider call was made. Add --execute for the explicit run.\n");
    return;
  }

  const cleanupPlaywright = preparePlaywrightResource(repositoryRoot);
  try {
    const cargo = cargoBuildInvocation();
    process.stdout.write("Building the production-path evaluation driver once...\n");
    checked(cargo.command, cargo.args, { stdio: "inherit" });
    const binary = evaluationBinary();
    requireFact(fs.existsSync(binary), `evaluation binary is missing at ${binary}`);
    const plan = evaluationPlan(prepared, options);
    const runs = checkpointRuns(prepared, plan);
    process.stdout.write(
      `Running ${plan.entries.length} provider-backed Direct/Fast/Auto/Pro samples; ${runs.size} resumed.\n`
    );
    let report = null;
    for (const [index, entry] of plan.entries.entries()) {
      const key = `${entry.caseId}/${entry.treatment}/r${entry.replicate}`;
      if (runs.has(key)) continue;
      process.stdout.write(`[${index + 1}/${plan.entries.length}] ${key}\n`);
      const result = runSingleEvaluation(binary, prepared, entry);
      runs.set(key, result.run);
      report = mergeRunCheckpoint(
        report || result.report,
        result.run,
        plan,
        prepared.replicates,
        [...runs.values()]
      );
      writePrivateJson(prepared.outputs.raw, report);
    }
    requireFact(runs.size === plan.entries.length, "evaluation checkpoint is incomplete");
    if (!report) {
      report = JSON.parse(fs.readFileSync(prepared.outputs.raw, "utf8"));
    }
    validatePostflight(prepared);
    const analyzer = analyzerInvocation(prepared);
    checked(analyzer.command, analyzer.args, { stdio: "inherit" });
  } finally {
    cleanupPlaywright();
  }
  process.stdout.write("Evaluation complete; only the sanitized JSON and Markdown are publishable.\n");
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`Agent real-world evaluation failed: ${error.message}\n`);
    process.exitCode = 1;
  }
}
