import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const target = process.env.CINDX_DESKTOP_TARGET ?? "aarch64-apple-darwin";
const cargo = process.env.CARGO ?? "cargo";
const result = spawnSync(
  cargo,
  [
    "tree",
    "--locked",
    "--manifest-path",
    "apps/desktop/src-tauri/Cargo.toml",
    "--target",
    target,
    "--edges",
    "normal",
    "--prefix",
    "none",
  ],
  { cwd: repoRoot, encoding: "utf8" }
);

if (result.status !== 0) {
  process.stderr.write(result.stderr || result.stdout);
  process.exit(result.status ?? 1);
}

const reqwestVersions = new Set();
for (const line of result.stdout.split("\n")) {
  const match = line.trim().match(/^reqwest v([^\s]+)/);
  if (match) reqwestVersions.add(match[1]);
}

if (reqwestVersions.size !== 1 || ![...reqwestVersions][0].startsWith("0.12.")) {
  throw new Error(
    `macOS desktop must compile one reqwest 0.12.x stack; found ${
      [...reqwestVersions].join(", ") || "none"
    }`
  );
}

for (const forbidden of ["aws-lc-rs v", "aws-lc-sys v"]) {
  if (result.stdout.includes(forbidden)) {
    throw new Error(`macOS desktop dependency tree contains forbidden ${forbidden.trim()}`);
  }
}

console.log(
  `Desktop dependency policy passed (${target}, reqwest ${[...reqwestVersions][0]}, ring TLS).`
);
