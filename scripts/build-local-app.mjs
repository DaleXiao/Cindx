import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { nextCindxVersion } from "./versioning.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = path.join(repoRoot, "apps", "desktop");
const tauriRoot = path.join(desktopRoot, "src-tauri");
const targetTriple = "aarch64-apple-darwin";
const sourceRevisionResult = spawnSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8"
});
const sourceRevision = sourceRevisionResult.stdout?.trim();
if (sourceRevisionResult.status !== 0 || !/^[0-9a-f]{40}$/.test(sourceRevision ?? "")) {
  throw new Error("A full Git source revision is required for a reproducible Cindx build");
}
const sourceStatusResult = spawnSync(
  "git",
  ["status", "--porcelain=v1", "--untracked-files=all"],
  { cwd: repoRoot, encoding: "utf8" }
);
if (sourceStatusResult.status !== 0 || sourceStatusResult.stdout?.trim()) {
  throw new Error("A clean Git source tree is required for a reproducible Cindx build");
}
const args = new Set(process.argv.slice(2));
const skipTests = args.has("--skip-tests");
const installApp = !args.has("--no-install");
const ephemeralTarget = args.has("--ephemeral-target");
if (args.has("--source-version")) {
  throw new Error("--source-version is no longer supported; every local build advances the source version");
}
const configuredTargetRoot = process.env.CARGO_TARGET_DIR?.trim();
const targetRoot = ephemeralTarget
  ? fs.mkdtempSync(path.join(os.tmpdir(), "cindx-build-target-"))
  : configuredTargetRoot
    ? path.resolve(repoRoot, configuredTargetRoot)
    : path.join(tauriRoot, "target");
const versionPaths = [
  path.join(desktopRoot, "package.json"),
  path.join(desktopRoot, "package-lock.json"),
  path.join(tauriRoot, "tauri.conf.json"),
  path.join(tauriRoot, "Cargo.toml"),
  path.join(tauriRoot, "Cargo.lock"),
  path.join(repoRoot, "docs", "CURRENT.md")
];
const originals = new Map(
  versionPaths.map((filePath) => [filePath, fs.readFileSync(filePath, "utf8")])
);
const stableToolchainBin = path.join(
  os.homedir(),
  ".rustup",
  "toolchains",
  "stable-aarch64-apple-darwin",
  "bin"
);
const buildEnv = {
  ...process.env,
  CINDX_SOURCE_REVISION: sourceRevision,
  CARGO_TARGET_DIR: targetRoot,
  PATH: [
    path.join(os.homedir(), ".cargo", "bin"),
    "/opt/homebrew/opt/rustup/bin",
    stableToolchainBin,
    process.env.PATH
  ]
    .filter(Boolean)
    .join(path.delimiter)
};
let versionsRestored = false;
let buildCompleted = false;

function restoreVersions() {
  if (versionsRestored) return;
  originals.forEach((content, filePath) => fs.writeFileSync(filePath, content));
  versionsRestored = true;
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? buildEnv,
    encoding: options.encoding,
    stdio: options.encoding ? "pipe" : "inherit"
  });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0 && !options.allowFailure) {
    throw new Error(`${command} exited with status ${result.status ?? 1}`);
  }
  return result;
}

function processIsRunning(name) {
  return run("pgrep", ["-x", name], { encoding: "utf8", allowFailure: true }).status === 0;
}

function waitForProcessExit(name, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  const sleeper = new Int32Array(new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT));
  while (processIsRunning(name) && Date.now() < deadline) {
    Atomics.wait(sleeper, 0, 0, 100);
  }
  if (processIsRunning(name)) {
    throw new Error(`${name} did not exit after SIGTERM; close Cindx before installing`);
  }
}

function install(outputApp) {
  const destination = "/Applications/Cindx.app";
  // Build outputs share the installed app's name and bundle id. Stop every copy so
  // LaunchServices cannot reactivate a stale target/dist process after installation.
  run("pkill", ["-x", "cindx-desktop"], { allowFailure: true });
  waitForProcessExit("cindx-desktop");
  fs.rmSync(destination, { recursive: true, force: true });
  run("/usr/bin/ditto", [outputApp, destination]);
  run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", destination]);
  process.stdout.write(`Installed ${destination}\n`);
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    if (!buildCompleted) restoreVersions();
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}

const sourceVersion = JSON.parse(
  originals.get(path.join(tauriRoot, "tauri.conf.json"))
).version;
const requestedBuildOrdinal = process.env.CINDX_BUILD_NUMBER || undefined;
const version = nextCindxVersion(sourceVersion, requestedBuildOrdinal);
const builtApp = path.join(
  targetRoot,
  targetTriple,
  "release",
  "bundle",
  "macos",
  "Cindx.app"
);
const outputRoot = path.join(repoRoot, "dist");
const outputApp = path.join(outputRoot, "Cindx.app");
const outputArchive = path.join(outputRoot, `Cindx-${version}-macOS-arm64.zip`);

try {
  run(process.execPath, [
    path.join(repoRoot, "scripts", "stamp-build-version.mjs"),
    "--version",
    version
  ]);
  run(process.execPath, [path.join(repoRoot, "scripts", "check-docs.mjs")]);
  run(process.execPath, ["--test", path.join(repoRoot, "scripts", "versioning.test.mjs")]);
  run(process.execPath, [path.join(repoRoot, "scripts", "check-desktop-structure.mjs")]);
  run(process.execPath, [path.join(repoRoot, "scripts", "check-desktop-layout.mjs")]);
  run("rustup", ["target", "add", targetTriple]);
  if (!skipTests) {
    run(process.execPath, [path.join(repoRoot, "scripts", "test-browser-sidecar.mjs")]);
    run(process.execPath, [path.join(repoRoot, "scripts", "test-computer-sidecar.mjs")]);
    run("npm", ["test"], { cwd: desktopRoot });
    run("cargo", ["test", "--workspace", "--locked"]);
    run("cargo", ["test", "--manifest-path", path.join(tauriRoot, "Cargo.toml"), "--locked"]);
    run(process.execPath, [
      path.join(repoRoot, "scripts", "run-quality-gates.mjs"),
      "--profile",
      "shipping-performance",
      "--report",
      "target/shipping-performance-report.json"
    ]);
  }
  run(
    path.join(desktopRoot, "node_modules", ".bin", "tauri"),
    ["build", "--target", targetTriple, "--bundles", "app"],
    { cwd: desktopRoot }
  );
  run("codesign", [
    "--force",
    "--deep",
    "--sign",
    "-",
    "--identifier",
    "app.cindx.desktop",
    builtApp
  ]);
  run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", builtApp]);

  const probeRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-local-probe-"));
  try {
    const probeData = path.join(probeRoot, "data");
    run(path.join(builtApp, "Contents", "MacOS", "cindx-desktop"), [], {
      env: {
        ...buildEnv,
        HOME: probeRoot,
        CINDX_DATA_DIR: probeData,
        CINDX_STARTUP_PROBE: "1"
      }
    });
    if (!fs.existsSync(path.join(probeData, "state.sqlite3"))) {
      throw new Error("Clean-machine startup probe did not create state.sqlite3");
    }
    const startupLog = fs.readFileSync(path.join(probeData, "startup.log"), "utf8");
    if (!startupLog.includes("startup probe completed")) {
      throw new Error("Clean-machine startup probe did not complete");
    }

    const failureData = path.join(probeRoot, "failure-data");
    fs.mkdirSync(path.join(failureData, "state.sqlite3"), { recursive: true });
    const failureResult = run(
      path.join(builtApp, "Contents", "MacOS", "cindx-desktop"),
      [],
      {
        env: {
          ...buildEnv,
          HOME: probeRoot,
          CINDX_DATA_DIR: failureData,
          CINDX_STARTUP_PROBE: "1"
        },
        encoding: "utf8",
        allowFailure: true
      }
    );
    if (failureResult.status === 0) {
      throw new Error("Persistent-state failure probe unexpectedly started Cindx");
    }
    const failureLog = fs.readFileSync(
      path.join(failureData, "startup.log"),
      "utf8"
    );
    if (
      !failureLog.includes("persistent state unavailable; startup aborted") ||
      failureLog.includes("startup probe completed")
    ) {
      throw new Error("Persistent-state failure probe did not fail closed");
    }
  } finally {
    fs.rmSync(probeRoot, { recursive: true, force: true });
  }

  fs.mkdirSync(outputRoot, { recursive: true });
  fs.rmSync(outputApp, { recursive: true, force: true });
  fs.rmSync(outputArchive, { force: true });
  run("/usr/bin/ditto", [builtApp, outputApp]);
  run("/usr/bin/ditto", [
    "-c",
    "-k",
    "--sequesterRsrc",
    "--keepParent",
    outputApp,
    outputArchive
  ]);
  run("shasum", ["-a", "256", outputArchive]);
  buildCompleted = true;
  if (installApp) install(outputApp);
  process.stdout.write(`Local Cindx build ${version}\n${outputApp}\n${outputArchive}\n`);
} finally {
  if (!buildCompleted) restoreVersions();
  if (ephemeralTarget) fs.rmSync(targetRoot, { recursive: true, force: true });
}
