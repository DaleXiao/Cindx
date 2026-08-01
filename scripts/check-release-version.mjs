import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = path.join(repoRoot, "apps", "desktop");

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
}

function packageVersion(toml) {
  return toml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1] ?? null;
}

function lockedPackageVersion(lock) {
  return lock.match(/\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/)?.[1] ?? null;
}

function documentedVersion(markdown) {
  return markdown.match(/^Current application version: `([^`]+)`$/m)?.[1] ?? null;
}

const packageJson = readJson("apps/desktop/package.json");
const packageLock = readJson("apps/desktop/package-lock.json");
const tauriConfig = readJson("apps/desktop/src-tauri/tauri.conf.json");
const cargoToml = fs.readFileSync(path.join(desktopRoot, "src-tauri", "Cargo.toml"), "utf8");
const cargoLock = fs.readFileSync(path.join(desktopRoot, "src-tauri", "Cargo.lock"), "utf8");
const currentDoc = fs.readFileSync(path.join(repoRoot, "docs", "CURRENT.md"), "utf8");
const version = tauriConfig.version;
const versions = new Map([
  ["package.json", packageJson.version],
  ["package-lock.json", packageLock.version],
  ["package-lock root package", packageLock.packages?.[""]?.version],
  ["Cargo.toml", packageVersion(cargoToml)],
  ["Cargo.lock", lockedPackageVersion(cargoLock)],
  ["docs/CURRENT.md", documentedVersion(currentDoc)]
]);

const mismatches = [...versions].filter(([, candidate]) => candidate !== version);
if (mismatches.length > 0) {
  const details = mismatches.map(([name, candidate]) => `${name}=${candidate}`).join(", ");
  throw new Error(`Cindx version files do not match ${version}: ${details}`);
}

if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error(`Cindx version is not semantic: ${version}`);
}

const tag = process.argv[2];
if (tag && tag !== `v${version}`) {
  throw new Error(`Release tag ${tag} does not match committed version v${version}`);
}

process.stdout.write(`Cindx release version ${version}${tag ? ` matches ${tag}` : ""}\n`);
