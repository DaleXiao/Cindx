import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const defaultSuite = path.join(repositoryRoot, "benchmarks", "agent", "realworld-v4.json");
const executionOrderProtocol = "cyclic_latin_square_v1";

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
      "--treatments": "treatments",
      "--auto-profile": "autoProfile",
      "--pro-profile": "proProfile"
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

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function sha256Json(value) {
  return crypto.createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export function validateProfileArtifact(root, file, expectedEffort) {
  const supplied = path.resolve(file);
  requireFact(fs.existsSync(supplied), `--${expectedEffort}-profile must reference a file`);
  const resolved = fs.realpathSync(supplied);
  requireFact(
    !isWithin(fs.realpathSync(root), resolved),
    `--${expectedEffort}-profile must remain outside the Git checkout`
  );
  requireFact(fs.statSync(resolved).isFile(), `--${expectedEffort}-profile must reference a file`);
  const bytes = fs.readFileSync(resolved);
  let snapshot;
  try {
    snapshot = JSON.parse(bytes);
  } catch (error) {
    throw new Error(`--${expectedEffort}-profile is not valid JSON: ${error.message}`);
  }
  requireFact(snapshot?.schema === "cindx.prompt-profile-snapshot.v1", `--${expectedEffort}-profile schema mismatch`);
  requireFact(snapshot.effort === expectedEffort, `--${expectedEffort}-profile effort mismatch`);
  requireFact(snapshot.genome && typeof snapshot.genome === "object", `--${expectedEffort}-profile genome is missing`);
  requireFact(/^[0-9a-f]{64}$/.test(snapshot.candidate_sha256), `--${expectedEffort}-profile candidate hash is invalid`);
  requireFact(typeof snapshot.stable_profile_id === "string" && snapshot.stable_profile_id.trim(), `--${expectedEffort}-profile stable profile is missing`);
  requireFact(typeof snapshot.evolution_method === "string" && snapshot.evolution_method.length > 0, `--${expectedEffort}-profile evolution method is missing`);
  requireFact(typeof snapshot.promotion_gate_protocol === "string" && snapshot.promotion_gate_protocol.length > 0, `--${expectedEffort}-profile promotion protocol is missing`);
  return {
    mode: "frozen_profile",
    path: resolved,
    file_sha256: sha256(bytes),
    artifact_sha256: sha256(Buffer.from(JSON.stringify(snapshot))),
    candidate_sha256: snapshot.candidate_sha256,
    evolution_method: snapshot.evolution_method
  };
}

function profileArtifacts(options) {
  return Object.fromEntries(
    [
      ["auto", options.autoProfile],
      ["pro", options.proProfile]
    ].map(([effort, file]) => [
      effort,
      file
        ? validateProfileArtifact(repositoryRoot, file, effort)
        : {
            mode: "fresh_seed",
            path: null,
            file_sha256: null,
            artifact_sha256: null,
            candidate_sha256: null,
            evolution_method: null
          }
    ])
  );
}

function publicProfileArtifacts(artifacts) {
  return Object.fromEntries(
    Object.entries(artifacts).map(([effort, artifact]) => [
      effort,
      {
        mode: artifact.mode,
        file_sha256: artifact.file_sha256,
        artifact_sha256: artifact.artifact_sha256,
        candidate_sha256: artifact.candidate_sha256,
        evolution_method: artifact.evolution_method
      }
    ])
  );
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
  const configuredModels = Object.fromEntries(
    [
      ["default", "model"],
      ["conductor", "conductor_model"],
      ["planner", "planner_model"],
      ["executor", "executor_model"],
      ["reviewer", "reviewer_model"],
      ["summarizer", "summarizer_model"],
      ["embedding", "embedding_model"]
    ].map(([role, key]) => [role, values.get(key) || ""])
  );
  const relevantConfiguration = Object.fromEntries(
    [
      "provider_id",
      "provider_resource",
      "base_url",
      "api_key",
      "model",
      "conductor_model",
      "planner_model",
      "executor_model",
      "reviewer_model",
      "summarizer_model",
      "embedding_model",
      "collaboration_policy",
      "prompt_evolution_enabled",
      "context_window_tokens",
      "agent_system_prompt_hex"
    ].map((key) => [key, values.get(key) || ""])
  );
  return {
    binding: {
      provider_id: values.get("provider_id"),
      provider_endpoint: values.get("base_url"),
      configured_models: configuredModels
    },
    sha256: sha256Json(relevantConfiguration)
  };
}

function recordsEqual(left, right) {
  return JSON.stringify(Object.entries(left || {}).sort()) ===
    JSON.stringify(Object.entries(right || {}).sort());
}

function validateProviderBinding(actual, expected, label) {
  requireFact(actual.provider_id === expected.provider_id, `${label} provider id changed`);
  requireFact(actual.provider_endpoint === expected.provider_endpoint, `${label} provider endpoint changed`);
  requireFact(
    recordsEqual(actual.configured_models, expected.configured_models),
    `${label} configured models changed`
  );
}

function configuredProvider() {
  return parseProviderConfig(
    path.join(process.env.HOME, "Library", "Application Support", "Cindx", "provider.conf")
  );
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
    fs.unlinkSync(destination);
    try {
      fs.rmdirSync(path.dirname(destination));
    } catch {
      // A pre-existing node_modules directory is left intact.
    }
  };
}

export function validatePreflight({ suite, gitHead, status, requestedReplicates }) {
  requireFact(suite.schema === "cindx.agent-realworld-suite.v3", "suite schema mismatch");
  requireFact(
    JSON.stringify(suite.treatments) === JSON.stringify(["direct", "fast", "auto", "pro"]),
    "suite treatments must be Direct/Fast/Auto/Pro in frozen order"
  );
  requireFact(
    suite.execution_order?.protocol === executionOrderProtocol &&
      JSON.stringify(suite.execution_order.base_treatments) ===
        JSON.stringify(suite.treatments),
    "suite execution-order contract mismatch"
  );
  requireFact(
    suite.promotion_v1?.schema === "cindx.agent-realworld-promotion.v1" &&
      suite.promotion_v1.baseline === "fast",
    "suite promotion contract mismatch"
  );
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
  const artifacts = profileArtifacts(options);
  const provider = configuredProvider();
  return {
    ...facts,
    suite,
    suitePath,
    suiteSha256: sha256File(suitePath),
    gitHead,
    providerBinding: provider.binding,
    providerConfigSha256: provider.sha256,
    profileArtifacts: artifacts,
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

export function evaluationEnvironment(base, prepared, entry, output, tempRoot) {
  const playwrightModules = path.join(repositoryRoot, "apps", "desktop", "node_modules");
  const nodePath = base.NODE_PATH
    ? `${playwrightModules}${path.delimiter}${base.NODE_PATH}`
    : playwrightModules;
  const environment = {
    ...base,
    NODE_PATH: nodePath,
    CINDX_EVAL_GIT_COMMIT: prepared.gitHead,
    CINDX_AGENT_REALWORLD_SUITE: prepared.suitePath,
    CINDX_AGENT_REALWORLD_OUTPUT: output,
    CINDX_AGENT_REALWORLD_REPLICATES: String(prepared.replicates),
    CINDX_AGENT_REALWORLD_REPLICATE_INDEX: String(entry.replicate),
    CINDX_AGENT_REALWORLD_CASES: entry.caseId,
    CINDX_AGENT_REALWORLD_TREATMENTS: entry.treatment,
    CINDX_AGENT_REALWORLD_EXECUTION_INDEX: String(entry.executionIndex),
    CINDX_AGENT_REALWORLD_TREATMENT_POSITION: String(entry.treatmentPosition),
    CINDX_AGENT_REALWORLD_EXECUTION_ORDER_PROTOCOL: executionOrderProtocol,
    CINDX_AGENT_REALWORLD_PLAN_SHA256: prepared.executionPlanSha256,
    CINDX_AGENT_REALWORLD_TEMP_ROOT: tempRoot,
    CINDX_AGENT_REALWORLD_DATA_DIR: path.join(tempRoot, ".cindx-eval-data")
  };
  delete environment.CINDX_AGENT_REALWORLD_PROFILE_PATH;
  delete environment.CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256;
  delete environment.CINDX_AGENT_REALWORLD_PROFILE_MODE;
  const artifact = prepared.profileArtifacts?.[entry.treatment];
  if (artifact) {
    environment.CINDX_AGENT_REALWORLD_PROFILE_MODE = artifact.mode;
    if (artifact.path) {
      environment.CINDX_AGENT_REALWORLD_PROFILE_PATH = artifact.path;
      environment.CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256 = artifact.artifact_sha256;
    }
  }
  return environment;
}

function selectedInSuite(values, requested, label) {
  if (!requested) return [...values];
  const requestedValues = new Set(requested.split(",").map((value) => value.trim()).filter(Boolean));
  const selected = values.filter((value) => requestedValues.has(value));
  requireFact(selected.length === requestedValues.size && selected.length > 0, `${label} selection is invalid`);
  return selected;
}

export function evaluationPlan(prepared, options) {
  const cases = selectedInSuite(
    prepared.suite.cases.map((testCase) => testCase.id),
    options.cases,
    "case"
  );
  const treatments = selectedInSuite(prepared.suite.treatments, options.treatments, "treatment");
  const fullEntries = [];
  let blockIndex = 0;
  for (let replicate = 1; replicate <= prepared.replicates; replicate += 1) {
    for (const testCase of prepared.suite.cases) {
      const rotation = blockIndex % prepared.suite.treatments.length;
      for (let position = 0; position < prepared.suite.treatments.length; position += 1) {
        fullEntries.push({
          replicate,
          caseId: testCase.id,
          treatment:
            prepared.suite.treatments[
              (position + rotation) % prepared.suite.treatments.length
            ],
          executionIndex: fullEntries.length + 1,
          treatmentPosition: position + 1
        });
      }
      blockIndex += 1;
    }
  }
  const selectedCases = new Set(cases);
  const selectedTreatments = new Set(treatments);
  const entries = fullEntries.filter(
    (entry) => selectedCases.has(entry.caseId) && selectedTreatments.has(entry.treatment)
  );
  const profileArtifacts = publicProfileArtifacts(prepared.profileArtifacts);
  const digestPayload = {
    protocol: executionOrderProtocol,
    suite_sha256: prepared.suiteSha256,
    git_commit: prepared.gitHead,
    replicates: prepared.replicates,
    cases,
    treatments,
    provider_config_sha256: prepared.providerConfigSha256,
    profile_artifacts: profileArtifacts,
    entries
  };
  return {
    protocol: executionOrderProtocol,
    sha256: sha256Json(digestPayload),
    providerConfigSha256: prepared.providerConfigSha256,
    profileArtifacts,
    cases,
    treatments,
    entries
  };
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
      postcondition_receipts: [],
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

export function validateCheckpoint(prepared, plan, raw) {
  requireFact(raw?.schema === "cindx.agent-realworld-raw.v3", "checkpoint schema mismatch");
  requireFact(raw.git_commit === prepared.gitHead, "checkpoint Git commit mismatch");
  requireFact(raw.suite_sha256 === prepared.suiteSha256, "checkpoint suite hash mismatch");
  requireFact(raw.requested_replicates === prepared.replicates, "checkpoint replicate count mismatch");
  requireFact(raw.execution_order_protocol === plan.protocol, "checkpoint execution-order protocol mismatch");
  requireFact(raw.execution_plan_sha256 === plan.sha256, "checkpoint execution plan mismatch");
  requireFact(raw.provider_config_sha256 === plan.providerConfigSha256, "checkpoint provider configuration mismatch");
  validateProviderBinding(raw, prepared.providerBinding, "checkpoint");
  requireFact(JSON.stringify(raw.profile_artifacts) === JSON.stringify(plan.profileArtifacts), "checkpoint profile artifact mismatch");
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
    const entry = plan.entries.find((candidate) =>
      candidate.caseId === run.case_id &&
      candidate.treatment === run.treatment &&
      candidate.replicate === run.replicate
    );
    requireFact(run.execution_index === entry.executionIndex, `${key}: checkpoint execution index mismatch`);
    requireFact(run.treatment_position === entry.treatmentPosition, `${key}: checkpoint treatment position mismatch`);
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
    execution_order_protocol: plan.protocol,
    execution_plan_sha256: plan.sha256,
    provider_config_sha256: plan.providerConfigSha256,
    profile_artifacts: plan.profileArtifacts,
    runs: plan.entries.map((entry) => runs.get(`${entry.caseId}/${entry.treatment}/r${entry.replicate}`)).filter(Boolean)
  };
}

function terminateRunProcesses(tempRoot) {
  if (process.platform !== "darwin") return;
  spawnSync("pkill", ["-TERM", "-f", tempRoot], { stdio: "ignore" });
  spawnSync("pkill", ["-KILL", "-f", tempRoot], { stdio: "ignore" });
}

function runSingleEvaluation(binary, prepared, entry) {
  const provider = configuredProvider();
  requireFact(provider.sha256 === prepared.providerConfigSha256, "provider configuration changed before execution");
  validateProviderBinding(provider.binding, prepared.providerBinding, "provider configuration");
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
    requireFact(report.execution_order_protocol === executionOrderProtocol, "run checkpoint execution-order protocol mismatch");
    requireFact(report.execution_plan_sha256 === prepared.executionPlanSha256, "run checkpoint execution plan mismatch");
    validateProviderBinding(report, prepared.providerBinding, "run checkpoint");
    requireFact(report.runs?.length === 1, "single-run process produced an invalid run count");
    let run = report.runs[0];
    requireFact(run.execution_index === entry.executionIndex, "single-run execution index mismatch");
    requireFact(run.treatment_position === entry.treatmentPosition, "single-run treatment position mismatch");
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
  const provider = configuredProvider();
  requireFact(provider.sha256 === prepared.providerConfigSha256, "provider configuration changed during evaluation");
  validateProviderBinding(provider.binding, prepared.providerBinding, "provider configuration");
  requireFact(checked("git", ["rev-parse", "HEAD"]) === prepared.gitHead, "Git HEAD changed during evaluation");
  requireFact(!checked("git", ["status", "--porcelain"]).trim(), "worktree changed during evaluation");
  const raw = JSON.parse(fs.readFileSync(prepared.outputs.raw, "utf8"));
  requireFact(raw.git_commit === prepared.gitHead, "raw evidence is not bound to evaluated HEAD");
  requireFact(raw.suite_sha256 === prepared.suiteSha256, "raw evidence suite hash drifted");
  requireFact(raw.execution_plan_sha256 === prepared.executionPlanSha256, "raw evidence plan drifted");
  requireFact(raw.provider_config_sha256 === prepared.providerConfigSha256, "raw provider configuration drifted");
  validateProviderBinding(raw, prepared.providerBinding, "raw evidence");
}

function usage() {
  return [
    "Usage: node scripts/run-agent-realworld.mjs [--execute] \\",
    "  --raw PATH --sanitized PATH --markdown PATH [--suite PATH] [--replicates N] \\",
    "  [--cases id,id] [--treatments direct,fast,auto,pro] \\",
    "  [--auto-profile PRIVATE_PATH] [--pro-profile PRIVATE_PATH]",
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
    prepared.executionPlanSha256 = plan.sha256;
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
