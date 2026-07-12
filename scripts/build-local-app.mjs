import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const desktopRoot = path.join(repoRoot, "apps", "desktop");
const tauriRoot = path.join(desktopRoot, "src-tauri");
const targetTriple = "aarch64-apple-darwin";
const args = new Set(process.argv.slice(2));
const skipTests = args.has("--skip-tests");
const installApp = !args.has("--no-install");
const versionPaths = [
  path.join(desktopRoot, "package.json"),
  path.join(desktopRoot, "package-lock.json"),
  path.join(tauriRoot, "tauri.conf.json"),
  path.join(tauriRoot, "Cargo.toml"),
  path.join(tauriRoot, "Cargo.lock")
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

function patchVersion(version) {
  const match = /^0\.0\.(\d+)$/.exec(version.trim());
  return match ? Number(match[1]) : 0;
}

function installedBuildNumber() {
  const plist = "/Applications/Cindx.app/Contents/Info.plist";
  if (!fs.existsSync(plist)) return 0;
  const result = run(
    "plutil",
    ["-extract", "CFBundleShortVersionString", "raw", plist],
    { encoding: "utf8", allowFailure: true }
  );
  return result.status === 0 ? patchVersion(result.stdout ?? "") : 0;
}

function nextBuildNumber() {
  const sourceVersion = JSON.parse(originals.get(path.join(tauriRoot, "tauri.conf.json"))).version;
  const counterPath = path.join(repoRoot, ".cindx", "local-build-number");
  const localCounter = fs.existsSync(counterPath)
    ? Number(fs.readFileSync(counterPath, "utf8").trim()) || 0
    : 0;
  const baseline = Math.max(patchVersion(sourceVersion), installedBuildNumber(), localCounter);
  const requested = Number(process.env.CINDX_BUILD_NUMBER);
  return Number.isSafeInteger(requested) && requested > baseline ? requested : baseline + 1;
}

function install(outputApp) {
  const destination = "/Applications/Cindx.app";
  run("pkill", ["-f", `${destination}/Contents/MacOS/cindx-desktop`], { allowFailure: true });
  fs.rmSync(destination, { recursive: true, force: true });
  run("/usr/bin/ditto", [outputApp, destination]);
  run("codesign", ["--verify", "--deep", "--strict", "--verbose=2", destination]);
  process.stdout.write(`Installed ${destination}\n`);
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    restoreVersions();
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}

const buildNumber = nextBuildNumber();
const version = `0.0.${buildNumber}`;
const counterPath = path.join(repoRoot, ".cindx", "local-build-number");
const builtApp = path.join(
  tauriRoot,
  "target",
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
  run(process.execPath, [path.join(repoRoot, "scripts", "stamp-build-version.mjs"), String(buildNumber)]);
  run(process.execPath, [path.join(repoRoot, "scripts", "check-desktop-structure.mjs")]);
  run(process.execPath, [path.join(repoRoot, "scripts", "check-desktop-layout.mjs")]);
  run("rustup", ["target", "add", targetTriple]);
  if (!skipTests) {
    run("cargo", ["test", "--workspace", "--locked"]);
    run("cargo", ["test", "--manifest-path", path.join(tauriRoot, "Cargo.toml"), "--locked"]);
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
  fs.mkdirSync(path.dirname(counterPath), { recursive: true });
  fs.writeFileSync(counterPath, `${buildNumber}\n`);
  if (installApp) install(outputApp);
  process.stdout.write(`Local Cindx build ${version}\n${outputApp}\n${outputArchive}\n`);
} finally {
  restoreVersions();
}
