import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function requireFile(relativePath) {
  if (!fs.existsSync(path.join(root, relativePath))) {
    failures.push(`missing required document: ${relativePath}`);
  }
}

function packageVersion(toml) {
  return toml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1];
}

function lockVersion(lock) {
  return lock.match(/\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/)?.[1];
}

function documentedVersion(markdown) {
  return markdown.match(/^Current application version: `([^`]+)`$/m)?.[1];
}

for (const file of [
  "AGENTS.md",
  "docs/README.md",
  "docs/CURRENT.md",
  "docs/ARCHITECTURE.md",
  "docs/AGENT_EVALUATION.md",
  "docs/QUALITY_GATES.md",
  "docs/evaluations/README.md",
  "docs/evaluations/archive/README.md"
]) {
  requireFile(file);
}

const packageJson = JSON.parse(read("apps/desktop/package.json"));
const packageLock = JSON.parse(read("apps/desktop/package-lock.json"));
const tauriConfig = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json"));
const cargoTomlVersion = packageVersion(read("apps/desktop/src-tauri/Cargo.toml"));
const cargoLockVersion = lockVersion(read("apps/desktop/src-tauri/Cargo.lock"));
const currentDocVersion = documentedVersion(read("docs/CURRENT.md"));
const versions = new Map([
  ["package.json", packageJson.version],
  ["package-lock.json", packageLock.version],
  ["package-lock root", packageLock.packages?.[""]?.version],
  ["tauri.conf.json", tauriConfig.version],
  ["desktop Cargo.toml", cargoTomlVersion],
  ["desktop Cargo.lock", cargoLockVersion],
  ["docs/CURRENT.md", currentDocVersion]
]);
const expectedVersion = tauriConfig.version;
for (const [label, version] of versions) {
  if (version !== expectedVersion) {
    failures.push(`${label} version ${version ?? "missing"} != ${expectedVersion}`);
  }
}

for (const obsolete of [
  "docs/MODULES.md",
  "docs/MVP_SPEC.md",
  "docs/POST_MVP_SPEC.md",
  "docs/ROADMAP.md",
  "docs/VISUAL_QA.md",
  "docs/diagram.html",
  "docs/handoffs"
]) {
  if (fs.existsSync(path.join(root, obsolete))) {
    failures.push(`obsolete documentation path returned: ${obsolete}`);
  }
}

const evaluationRoot = path.join(root, "docs/evaluations");
const allowedEvaluationRootFiles = new Set([
  "README.md",
  "CINDX_WORKFLOW_GEPA_V2_INVALID_0.2.25_2026-08-08.md",
  "CINDX_WORKFLOW_GEPA_V3_INVALID_0.2.25_2026-08-08.md",
  "CINDX_WORKFLOW_GEPA_V4_INVALID_0.2.25_2026-08-08.md",
  "CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.md",
  "CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json",
  "CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.md",
  "CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.json",
  "CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.md",
  "CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.json",
  "CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.md",
  "CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.json",
  "CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.md",
  "CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.json",
  "CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.md",
  "CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.json",
  "CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md",
  "CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.json",
  "CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md",
  "CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.json",
  "CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md",
  "CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md",
  "CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md",
  "CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.json",
  "CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md",
  "CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.json",
  "CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md",
  "CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.json",
  "CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md",
  "CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.json"
]);
for (const entry of fs.readdirSync(evaluationRoot, { withFileTypes: true })) {
  if (entry.isFile() && !allowedEvaluationRootFiles.has(entry.name)) {
    failures.push(`unindexed current evaluation file: docs/evaluations/${entry.name}`);
  }
}

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(target) : [target];
  });
}

for (const file of walk(evaluationRoot)) {
  if (!file.endsWith(".md") && !file.endsWith(".json")) continue;
  const source = fs.readFileSync(file, "utf8");
  if (/source commit[^\n]*`?unknown`?/i.test(source)) {
    failures.push(`evaluation has unknown source revision: ${path.relative(root, file)}`);
  }
}

const markdownFiles = [
  path.join(root, "README.md"),
  path.join(root, "AGENTS.md"),
  path.join(root, "releases/README.md"),
  ...walk(path.join(root, "docs")).filter((file) => file.endsWith(".md"))
];
const linkPattern = /\[[^\]]*\]\(([^)]+)\)/g;
for (const file of markdownFiles) {
  const source = fs.readFileSync(file, "utf8");
  for (const match of source.matchAll(linkPattern)) {
    const rawTarget = match[1].trim().replace(/^<|>$/g, "");
    if (
      !rawTarget ||
      rawTarget.startsWith("#") ||
      /^(?:https?:|mailto:)/i.test(rawTarget)
    ) {
      continue;
    }
    const relativeTarget = rawTarget.split("#", 1)[0];
    const resolved = path.resolve(path.dirname(file), decodeURIComponent(relativeTarget));
    if (!fs.existsSync(resolved)) {
      failures.push(
        `broken link in ${path.relative(root, file)}: ${rawTarget}`
      );
    }
  }
}

if (failures.length > 0) {
  process.stderr.write(`Documentation check failed:\n- ${failures.join("\n- ")}\n`);
  process.exit(1);
}

process.stdout.write(`Documentation baseline ${expectedVersion} is consistent.\n`);
