import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const harnessRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);

function option(name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : null;
}

function requiredPath(name) {
  const value = option(name);
  if (!value) throw new Error(`Missing required option ${name}`);
  return path.resolve(value);
}

function run(label, executable, commandArgs, options = {}) {
  process.stdout.write(`\n[paired-performance] ${label}\n`);
  const result = spawnSync(executable, commandArgs, {
    cwd: harnessRoot,
    env: options.env ?? process.env,
    stdio: "inherit"
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${label} failed with exit code ${result.status}`);
  }
}

function repositoryCommit(root) {
  const result = spawnSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8"
  });
  if (result.status !== 0) throw new Error(`Cannot resolve Git commit for ${root}`);
  return result.stdout.trim();
}

function requireCleanTrackedFiles(root) {
  const result = spawnSync("git", ["status", "--porcelain", "--untracked-files=no"], {
    cwd: root,
    encoding: "utf8"
  });
  if (result.status !== 0) throw new Error(`Cannot inspect Git state for ${root}`);
  if (result.stdout.trim()) throw new Error(`Tracked files are dirty in ${root}`);
}

function requireReportCommit(reportPath, expectedCommit) {
  const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));
  if (report.commit !== expectedCommit) {
    throw new Error(
      `${reportPath} measured ${report.commit ?? "no commit"}, expected ${expectedCommit}`
    );
  }
}

const baselineRoot = requiredPath("--baseline-root");
const candidateRoot = requiredPath("--candidate-root");
const outputDir = requiredPath("--output-dir");
requireCleanTrackedFiles(baselineRoot);
requireCleanTrackedFiles(candidateRoot);
const baselineCommit = repositoryCommit(baselineRoot);
const candidateCommit = repositoryCommit(candidateRoot);
if (baselineCommit === candidateCommit) {
  throw new Error("Baseline and candidate must resolve to different commits");
}

fs.mkdirSync(outputDir, { recursive: true });
const baselineReport = path.join(outputDir, "baseline.json");
const candidateReport = path.join(outputDir, "candidate.json");
const comparisonReport = path.join(outputDir, "comparison.json");
for (const report of [baselineReport, candidateReport, comparisonReport]) {
  fs.rmSync(report, { force: true });
}
const pairId = crypto.randomUUID();
const pairedEnv = {
  ...process.env,
  CINDX_PERFORMANCE_PAIR_ID: pairId,
  CINDX_PERFORMANCE_BUILD_PROFILE: "test"
};
const qualityGateRunner = path.join(harnessRoot, "scripts", "run-quality-gates.mjs");

for (const [label, root, report] of [
  ["baseline profile", baselineRoot, baselineReport],
  ["candidate profile", candidateRoot, candidateReport]
]) {
  run(
    label,
    process.execPath,
    [
      qualityGateRunner,
      "--repo-root",
      root,
      "--manifest-root",
      candidateRoot,
      "--profile",
      "paired-performance",
      "--report",
      report
    ],
    { env: pairedEnv }
  );
}
requireReportCommit(baselineReport, baselineCommit);
requireReportCommit(candidateReport, candidateCommit);

run("comparison", process.execPath, [
  path.join(harnessRoot, "scripts", "compare-performance-reports.mjs"),
  "--baseline",
  baselineReport,
  "--candidate",
  candidateReport,
  "--policy",
  path.join(candidateRoot, "benchmarks", "system", "performance-policy-v1.json"),
  "--report",
  comparisonReport
]);

process.stdout.write(`\nPaired performance reports: ${outputDir}\n`);
