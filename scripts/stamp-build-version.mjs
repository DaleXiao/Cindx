import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { cindxVersionOrdinal, nextCindxVersion, parseCindxVersion } from "./versioning.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const paths = {
  packageJson: path.join(root, "apps/desktop/package.json"),
  packageLock: path.join(root, "apps/desktop/package-lock.json"),
  tauriConfig: path.join(root, "apps/desktop/src-tauri/tauri.conf.json"),
  cargoToml: path.join(root, "apps/desktop/src-tauri/Cargo.toml"),
  cargoLock: path.join(root, "apps/desktop/src-tauri/Cargo.lock"),
  currentDoc: path.join(root, "docs/CURRENT.md")
};

const packageJson = JSON.parse(fs.readFileSync(paths.packageJson, "utf8"));
const packageLock = JSON.parse(fs.readFileSync(paths.packageLock, "utf8"));
const tauriConfig = JSON.parse(fs.readFileSync(paths.tauriConfig, "utf8"));
const cargoToml = fs.readFileSync(paths.cargoToml, "utf8");
const cargoLock = fs.readFileSync(paths.cargoLock, "utf8");
const currentDoc = fs.readFileSync(paths.currentDoc, "utf8");
const current = tauriConfig.version;
const cargoVersion = cargoToml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1];
const lockedVersion = cargoLock.match(
  /\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/
)?.[1];
const documentedVersion = currentDoc.match(
  /^Current application version: `([^`]+)`$/m
)?.[1];
const versions = [
  packageJson.version,
  packageLock.version,
  packageLock.packages?.[""]?.version,
  cargoVersion,
  lockedVersion,
  documentedVersion
];
if (versions.some((version) => version !== current)) {
  throw new Error(`Cindx version files are out of sync: ${[current, ...versions].join(", ")}`);
}
parseCindxVersion(current);
const explicitVersionIndex = process.argv.indexOf("--version");
const explicitVersion =
  explicitVersionIndex >= 0 ? process.argv[explicitVersionIndex + 1] : undefined;
const requestedOrdinal = process.argv[2]?.startsWith("--")
  ? process.env.GITHUB_RUN_NUMBER
  : process.argv[2] || process.env.GITHUB_RUN_NUMBER;
const next = explicitVersion
  ? (() => {
      parseCindxVersion(explicitVersion);
      if (cindxVersionOrdinal(explicitVersion) <= cindxVersionOrdinal(current)) {
        throw new Error(`Cindx build version must advance beyond ${current}: ${explicitVersion}`);
      }
      return explicitVersion;
    })()
  : nextCindxVersion(current, requestedOrdinal);
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
fs.writeFileSync(
  paths.currentDoc,
  currentDoc.replace(
    /^Current application version: `[^`]+`$/m,
    `Current application version: \`${next}\``
  )
);

process.stdout.write(`Stamped Cindx version ${next}\n`);
