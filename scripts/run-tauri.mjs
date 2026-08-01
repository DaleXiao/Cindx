import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = path.join(repoRoot, "apps", "desktop");
const paths = {
  cargoLock: path.join(desktopRoot, "src-tauri", "Cargo.lock"),
  cargoToml: path.join(desktopRoot, "src-tauri", "Cargo.toml"),
  packageJson: path.join(desktopRoot, "package.json"),
  packageLock: path.join(desktopRoot, "package-lock.json"),
  tauriConfig: path.join(desktopRoot, "src-tauri", "tauri.conf.json"),
  currentDoc: path.join(repoRoot, "docs", "CURRENT.md")
};

function packageVersion(toml) {
  return toml.match(/^\[package\][\s\S]*?^version = "([^"]+)"$/m)?.[1] ?? null;
}

function replacePackageVersion(toml, version) {
  return toml.replace(
    /(^\[package\][\s\S]*?^version = ")[^"]+("$)/m,
    `$1${version}$2`
  );
}

function lockedPackageVersion(lock) {
  return lock.match(/\[\[package\]\]\nname = "cindx-desktop"\nversion = "([^"]+)"/)?.[1] ?? null;
}

function replaceLockedPackageVersion(lock, version) {
  return lock.replace(
    /(\[\[package\]\]\nname = "cindx-desktop"\nversion = ")[^"]+(")/,
    `$1${version}$2`
  );
}

function documentedVersion(markdown) {
  return markdown.match(/^Current application version: `([^`]+)`$/m)?.[1] ?? null;
}

function replaceDocumentedVersion(markdown, version) {
  return markdown.replace(
    /^Current application version: `[^`]+`$/m,
    `Current application version: \`${version}\``
  );
}

function nextPatchVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`Unsupported Cindx version: ${version}`);
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
}

function bumpDesktopVersion() {
  const originals = Object.fromEntries(
    Object.entries(paths).map(([name, filePath]) => [name, fs.readFileSync(filePath, "utf8")])
  );
  const packageJson = JSON.parse(originals.packageJson);
  const packageLock = JSON.parse(originals.packageLock);
  const tauriConfig = JSON.parse(originals.tauriConfig);
  const cargoToml = originals.cargoToml;
  const cargoLock = originals.cargoLock;
  const current = tauriConfig.version;
  const versions = [
    packageJson.version,
    packageLock.version,
    packageLock.packages?.[""]?.version,
    packageVersion(cargoToml),
    lockedPackageVersion(cargoLock),
    documentedVersion(originals.currentDoc)
  ];
  if (versions.some((version) => version !== current)) {
    throw new Error(`Cindx version files are out of sync: ${[current, ...versions].join(", ")}`);
  }

  const next = nextPatchVersion(current);
  packageJson.version = next;
  packageLock.version = next;
  packageLock.packages[""].version = next;
  tauriConfig.version = next;
  fs.writeFileSync(paths.packageJson, `${JSON.stringify(packageJson, null, 2)}\n`);
  fs.writeFileSync(paths.packageLock, `${JSON.stringify(packageLock, null, 2)}\n`);
  fs.writeFileSync(paths.tauriConfig, `${JSON.stringify(tauriConfig, null, 2)}\n`);
  fs.writeFileSync(paths.cargoToml, replacePackageVersion(cargoToml, next));
  fs.writeFileSync(paths.cargoLock, replaceLockedPackageVersion(cargoLock, next));
  fs.writeFileSync(paths.currentDoc, replaceDocumentedVersion(originals.currentDoc, next));
  process.stdout.write(`Cindx build version ${next}\n`);
  return () => {
    Object.entries(paths).forEach(([name, filePath]) => {
      fs.writeFileSync(filePath, originals[name]);
    });
    process.stderr.write(`Cindx build failed; restored version ${current}\n`);
  };
}

const args = process.argv.slice(2);
const rollbackDesktopVersion = args[0] === "build" ? bumpDesktopVersion() : null;

if (rollbackDesktopVersion) {
  const documentationCheck = spawnSync(
    process.execPath,
    [path.join(repoRoot, "scripts", "check-docs.mjs")],
    { cwd: repoRoot, stdio: "inherit" }
  );
  if (documentationCheck.error) {
    rollbackDesktopVersion();
    throw documentationCheck.error;
  }
  if ((documentationCheck.status ?? 1) !== 0) {
    rollbackDesktopVersion();
    process.exit(documentationCheck.status ?? 1);
  }
}

const executable = path.join(
  desktopRoot,
  "node_modules",
  ".bin",
  process.platform === "win32" ? "tauri.cmd" : "tauri"
);
const stableToolchain = path.join(
  os.homedir(),
  ".rustup",
  "toolchains",
  "stable-aarch64-apple-darwin",
  "bin"
);
const result = spawnSync(executable, args, {
  cwd: desktopRoot,
  env: {
    ...process.env,
    PATH: ["/opt/homebrew/opt/rustup/bin", stableToolchain, process.env.PATH]
      .filter(Boolean)
      .join(path.delimiter)
  },
  stdio: "inherit"
});

if (result.error) {
  rollbackDesktopVersion?.();
  throw result.error;
}
const exitCode = result.status ?? 1;
if (exitCode !== 0) rollbackDesktopVersion?.();
process.exit(exitCode);
