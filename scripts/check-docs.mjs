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

function walk(directory) {
  if (!fs.existsSync(directory)) return [];
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(target) : [target];
  });
}

function packageVersion(toml) {
  return toml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1];
}

function lockVersion(lock) {
  return lock.match(/\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/)?.[1];
}

function markedVersion(markdown, label) {
  return markdown.match(new RegExp("^" + label + ": `([^`]+)`$", "m"))?.[1];
}

const maintained = [
  "README.md",
  "AGENTS.md",
  "docs/CURRENT.md",
  "docs/ARCHITECTURE.md",
  "docs/DEVELOPMENT.md",
  "docs/EVALUATION.md",
  "docs/HANDOFF.md"
];
maintained.forEach(requireFile);

const allowedDocs = new Set(
  maintained.filter((file) => file.startsWith("docs/")).map((file) => file.slice(5))
);
for (const file of walk(path.join(root, "docs"))) {
  const relative = path.relative(path.join(root, "docs"), file);
  if (!allowedDocs.has(relative)) {
    failures.push(`unmaintained documentation file: docs/${relative}`);
  }
}

for (const obsolete of ["apps/desktop/README.md"]) {
  if (fs.existsSync(path.join(root, obsolete))) {
    failures.push(`obsolete documentation returned: ${obsolete}`);
  }
}

for (const entry of fs.readdirSync(root)) {
  if (/^Cindx-HANDOFF-.*\.md$/i.test(entry)) {
    failures.push(`versioned handoff is forbidden; update docs/HANDOFF.md: ${entry}`);
  }
}

for (const file of walk(path.join(root, "releases"))) {
  if (file.endsWith(".md") || file.endsWith(".json")) {
    failures.push(`release history belongs in GitHub Releases, not the source tree: ${path.relative(root, file)}`);
  }
}

const packageJson = JSON.parse(read("apps/desktop/package.json"));
const packageLock = JSON.parse(read("apps/desktop/package-lock.json"));
const tauriConfig = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json"));
const cargoTomlVersion = packageVersion(read("apps/desktop/src-tauri/Cargo.toml"));
const cargoLockVersion = lockVersion(read("apps/desktop/src-tauri/Cargo.lock"));
const currentVersion = markedVersion(read("docs/CURRENT.md"), "Current application version");
const handoffVersion = markedVersion(read("docs/HANDOFF.md"), "Current release version");
const expectedVersion = tauriConfig.version;

for (const [label, version] of new Map([
  ["package.json", packageJson.version],
  ["package-lock.json", packageLock.version],
  ["package-lock root", packageLock.packages?.[""]?.version],
  ["tauri.conf.json", tauriConfig.version],
  ["desktop Cargo.toml", cargoTomlVersion],
  ["desktop Cargo.lock", cargoLockVersion],
  ["docs/CURRENT.md", currentVersion],
  ["docs/HANDOFF.md", handoffVersion]
])) {
  if (version !== expectedVersion) {
    failures.push(`${label} version ${version ?? "missing"} != ${expectedVersion}`);
  }
}

const markdownFiles = maintained
  .filter((file) => file.endsWith(".md"))
  .map((file) => path.join(root, file));
const linkPattern = /\[[^\]]*\]\(([^)]+)\)/g;
for (const file of markdownFiles) {
  const source = fs.readFileSync(file, "utf8");
  for (const match of source.matchAll(linkPattern)) {
    const rawTarget = match[1].trim().replace(/^<|>$/g, "");
    if (!rawTarget || rawTarget.startsWith("#") || /^(?:https?:|mailto:)/i.test(rawTarget)) {
      continue;
    }
    const relativeTarget = rawTarget.split("#", 1)[0];
    const resolved = path.resolve(path.dirname(file), decodeURIComponent(relativeTarget));
    if (!fs.existsSync(resolved)) {
      failures.push(`broken link in ${path.relative(root, file)}: ${rawTarget}`);
    }
  }
}

if (failures.length > 0) {
  process.stderr.write(`Documentation check failed:\n- ${failures.join("\n- ")}\n`);
  process.exit(1);
}

process.stdout.write(
  `Documentation baseline ${expectedVersion} is consistent (${maintained.length} maintained files).\n`
);
