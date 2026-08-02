import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const defaultSuite = path.join(repositoryRoot, "benchmarks", "agent", "realworld-v1.json");

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

function cargoInvocation() {
  return {
    command: process.env.CARGO || "cargo",
    args: [
      "run",
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

function evaluationEnvironment(base, prepared, options) {
  const playwrightModules = path.join(repositoryRoot, "apps", "desktop", "node_modules");
  const nodePath = base.NODE_PATH
    ? `${playwrightModules}${path.delimiter}${base.NODE_PATH}`
    : playwrightModules;
  const environment = {
    ...base,
    NODE_PATH: nodePath,
    CINDX_EVAL_GIT_COMMIT: prepared.gitHead,
    CINDX_AGENT_REALWORLD_SUITE: prepared.suitePath,
    CINDX_AGENT_REALWORLD_OUTPUT: prepared.outputs.raw,
    CINDX_AGENT_REALWORLD_REPLICATES: String(prepared.replicates)
  };
  for (const key of ["CINDX_AGENT_REALWORLD_CASES", "CINDX_AGENT_REALWORLD_TREATMENTS"]) {
    delete environment[key];
  }
  if (options.cases) environment.CINDX_AGENT_REALWORLD_CASES = options.cases;
  if (options.treatments) environment.CINDX_AGENT_REALWORLD_TREATMENTS = options.treatments;
  return environment;
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
    const cargo = cargoInvocation();
    process.stdout.write("Running the provider-backed Direct/Fast/Auto/Pro matrix...\n");
    checked(cargo.command, cargo.args, {
      env: evaluationEnvironment(process.env, prepared, options),
      stdio: "inherit"
    });
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
