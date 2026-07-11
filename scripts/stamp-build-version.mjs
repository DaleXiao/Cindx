import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const runNumber = Number(process.argv[2] || process.env.GITHUB_RUN_NUMBER);
if (!Number.isSafeInteger(runNumber) || runNumber < 1) {
  throw new Error("A positive GitHub run number is required");
}

const paths = {
  packageJson: path.join(root, "apps/desktop/package.json"),
  packageLock: path.join(root, "apps/desktop/package-lock.json"),
  tauriConfig: path.join(root, "apps/desktop/src-tauri/tauri.conf.json"),
  cargoToml: path.join(root, "apps/desktop/src-tauri/Cargo.toml"),
  cargoLock: path.join(root, "apps/desktop/src-tauri/Cargo.lock")
};

const packageJson = JSON.parse(fs.readFileSync(paths.packageJson, "utf8"));
const packageLock = JSON.parse(fs.readFileSync(paths.packageLock, "utf8"));
const tauriConfig = JSON.parse(fs.readFileSync(paths.tauriConfig, "utf8"));
const cargoToml = fs.readFileSync(paths.cargoToml, "utf8");
const cargoLock = fs.readFileSync(paths.cargoLock, "utf8");
const current = tauriConfig.version;
const cargoVersion = cargoToml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1];
const lockedVersion = cargoLock.match(
  /\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/
)?.[1];
const versions = [
  packageJson.version,
  packageLock.version,
  packageLock.packages?.[""]?.version,
  cargoVersion,
  lockedVersion
];
if (versions.some((version) => version !== current)) {
  throw new Error(`Cindx version files are out of sync: ${[current, ...versions].join(", ")}`);
}
const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(current);
if (!match) throw new Error(`Unsupported Cindx version: ${current}`);

const next = `${match[1]}.${match[2]}.${Math.max(Number(match[3]) + 1, runNumber)}`;
packageJson.version = next;
packageLock.version = next;
packageLock.packages[""].version = next;
tauriConfig.version = next;

fs.writeFileSync(paths.packageJson, `${JSON.stringify(packageJson, null, 2)}\n`);
fs.writeFileSync(paths.packageLock, `${JSON.stringify(packageLock, null, 2)}\n`);
fs.writeFileSync(paths.tauriConfig, `${JSON.stringify(tauriConfig, null, 2)}\n`);
fs.writeFileSync(
  paths.cargoToml,
  cargoToml.replace(
    /(^\[package\][\s\S]*?^version = ")[^"]+("$)/m,
    `$1${next}$2`
  )
);
fs.writeFileSync(
  paths.cargoLock,
  cargoLock.replace(
    /(\[\[package\]\]\nname = "cindx-desktop"\nversion = ")[^"]+(")/,
    `$1${next}$2`
  )
);

process.stdout.write(`Stamped Cindx CI version ${next}\n`);
