import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const GPQA_SHA256 =
  "41d1213cd7a4998605a26c2798500652572007161b3a92817ba46b35befcd305";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const baselineContract = path.join(
  repositoryRoot,
  "benchmarks",
  "agent",
  "provider-baseline-v1.json"
);

function requiredValue(argv, index, flag) {
  const value = argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${flag} requires a path`);
  return value;
}

export function parseArguments(argv) {
  const options = { execute: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--execute") {
      options.execute = true;
      continue;
    }
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    const key = {
      "--gpqa": "gpqa",
      "--raw": "raw",
      "--sanitized": "sanitized",
      "--markdown": "markdown"
    }[argument];
    if (!key) throw new Error(`unknown argument ${argument}`);
    if (options[key]) throw new Error(`${argument} may be supplied only once`);
    options[key] = requiredValue(argv, index, argument);
    index += 1;
  }
  if (!options.help) {
    for (const key of ["gpqa", "raw", "sanitized", "markdown"]) {
      if (!options[key]) throw new Error(`--${key} is required`);
    }
  }
  return options;
}

export function isWithin(directory, candidate) {
  const relative = path.relative(path.resolve(directory), path.resolve(candidate));
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== "..");
}

function canonicalOutputPath(candidate) {
  const absolute = path.resolve(candidate);
  const parent = fs.realpathSync.native(path.dirname(absolute));
  return path.join(parent, path.basename(absolute));
}

export function validateOutputPaths(root, paths) {
  const canonicalRoot = fs.realpathSync.native(root);
  const resolved = Object.fromEntries(
    Object.entries(paths).map(([key, value]) => [key, canonicalOutputPath(value)])
  );
  for (const key of ["raw", "sanitized", "markdown"]) {
    if (isWithin(canonicalRoot, resolved[key])) {
      throw new Error(`--${key} must be outside the repository`);
    }
  }
  const identities = new Set(Object.values(resolved));
  if (identities.size !== Object.keys(resolved).length) {
    throw new Error("raw, sanitized, and markdown outputs must be distinct");
  }
  return resolved;
}

export function validatePreflightFacts({
  gpqaSha256,
  gitHead,
  trackedStatus,
  appVersion,
  expectedAppVersion
}) {
  if (gpqaSha256 !== GPQA_SHA256) {
    throw new Error(`GPQA file SHA-256 mismatch (expected ${GPQA_SHA256})`);
  }
  if (!/^[0-9a-f]{40}$/.test(gitHead)) {
    throw new Error("Git HEAD must be a full lowercase 40-character SHA");
  }
  if (trackedStatus.trim()) {
    throw new Error("tracked worktree changes must be committed before provider evaluation");
  }
  if (appVersion !== expectedAppVersion) {
    throw new Error(`app version ${appVersion || "missing"} does not match baseline ${expectedAppVersion}`);
  }
  return { gitHead, gpqaSha256 };
}

export function validatePostflightFacts({
  initialHead,
  finalHead,
  trackedStatus,
  rawGitCommit
}) {
  if (finalHead !== initialHead || rawGitCommit !== initialHead) {
    throw new Error("Git commit changed or raw evidence is not bound to the evaluated HEAD");
  }
  if (trackedStatus.trim()) {
    throw new Error("tracked worktree changed during provider evaluation");
  }
}

export function providerEnvironment(base, { gitHead, gpqa, raw }) {
  const environment = { ...base };
  for (const key of [
    "CINDX_GPQA_CASE_IDS",
    "CINDX_GPQA_CASE_LIMIT",
    "CINDX_GPQA_PER_DOMAIN",
    "CINDX_MRCR_LIMIT",
    "CINDX_MRCR_JSONS",
    "CINDX_EVAL_FROZEN_GEPA_AUTO_PATH",
    "CINDX_EVAL_FROZEN_GEPA_PRO_PATH"
  ]) {
    delete environment[key];
  }
  return {
    ...environment,
    CINDX_PROVIDER_BASELINE: "1",
    CINDX_EVAL_GIT_COMMIT: gitHead,
    CINDX_GPQA_CSV: gpqa,
    CINDX_GPQA_PER_DOMAIN: "4",
    CINDX_MRCR_JSONS: "",
    CINDX_MRCR_LIMIT: "0",
    CINDX_EXTERNAL_EVAL_OUTPUT: raw
  };
}

export function cargoInvocation(root) {
  return {
    command: "cargo",
    args: [
      "test",
      "--manifest-path",
      path.join(root, "apps", "desktop", "src-tauri", "Cargo.toml"),
      "--locked",
      "external_effect_eval_tests::provider_backed_fugu_external_effect_pilot",
      "--",
      "--ignored",
      "--exact",
      "--nocapture"
    ]
  };
}

export function analyzerInvocation(root, outputs) {
  return {
    command: "python3",
    args: [
      path.join(root, "scripts", "analyze-fugu-effect-eval.py"),
      outputs.raw,
      outputs.sanitized,
      "--baseline-contract",
      path.join(root, "benchmarks", "agent", "provider-baseline-v1.json"),
      "--markdown",
      outputs.markdown,
      "--title",
      "Cindx Provider-backed Matched Baseline"
    ]
  };
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

function desktopCargoVersion() {
  const manifest = fs.readFileSync(
    path.join(repositoryRoot, "apps", "desktop", "src-tauri", "Cargo.toml"),
    "utf8"
  );
  const match = manifest.match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (!match) throw new Error("desktop Cargo package version is missing");
  return match[1];
}

function preflight(options) {
  const gpqa = path.resolve(options.gpqa);
  if (!fs.statSync(gpqa).isFile()) throw new Error("--gpqa must reference a file");
  const outputs = validateOutputPaths(repositoryRoot, {
    raw: options.raw,
    sanitized: options.sanitized,
    markdown: options.markdown
  });
  const contract = JSON.parse(fs.readFileSync(baselineContract, "utf8"));
  const facts = validatePreflightFacts({
    gpqaSha256: sha256File(gpqa),
    gitHead: checked("git", ["rev-parse", "HEAD"]),
    trackedStatus: checked("git", ["status", "--porcelain", "--untracked-files=no"]),
    appVersion: desktopCargoVersion(),
    expectedAppVersion: contract.app_version
  });
  return { ...facts, gpqa, outputs };
}

function usage() {
  return [
    "Usage: node scripts/run-provider-baseline.mjs [--execute] \\",
    "  --gpqa PATH --raw PATH --sanitized PATH --markdown PATH",
    "",
    "Without --execute, only the dataset, Git, and output-boundary preflight runs."
  ].join("\n");
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArguments(argv);
  if (options.help) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  if (!fs.existsSync(baselineContract)) throw new Error("provider baseline contract is missing");
  const prepared = preflight(options);
  process.stdout.write(
    `Provider baseline preflight passed at ${prepared.gitHead.slice(0, 12)}; pinned GPQA hash verified.\n`
  );
  if (!options.execute) {
    process.stdout.write("No provider call was made. Add --execute to run the explicit evaluation.\n");
    return;
  }

  process.stdout.write("Running the provider-backed Direct/Auto/Pro baseline...\n");
  const cargo = cargoInvocation(repositoryRoot);
  checked(cargo.command, cargo.args, {
    env: providerEnvironment(process.env, {
      gitHead: prepared.gitHead,
      gpqa: prepared.gpqa,
      raw: prepared.outputs.raw
    }),
    stdio: "inherit"
  });
  const raw = JSON.parse(fs.readFileSync(prepared.outputs.raw, "utf8"));
  validatePostflightFacts({
    initialHead: prepared.gitHead,
    finalHead: checked("git", ["rev-parse", "HEAD"]),
    trackedStatus: checked("git", ["status", "--porcelain", "--untracked-files=no"]),
    rawGitCommit: raw.git_commit
  });
  process.stdout.write("Sanitizing and enforcing the frozen baseline contract...\n");
  const analyzer = analyzerInvocation(repositoryRoot, prepared.outputs);
  checked(analyzer.command, analyzer.args, { stdio: "inherit" });
  process.stdout.write("Provider baseline completed; only the sanitized reports are publishable.\n");
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`Provider baseline failed: ${error.message}\n`);
    process.exitCode = 1;
  }
}
