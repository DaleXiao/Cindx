import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const target = process.env.CINDX_DESKTOP_TARGET ?? "aarch64-apple-darwin";
const cargo = process.env.CARGO ?? "cargo";

function desktopDependencyTree(extraArgs = []) {
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
      ...extraArgs,
    ],
    { cwd: repoRoot, encoding: "utf8" }
  );

  if (result.status !== 0) {
    process.stderr.write(result.stderr || result.stdout);
    process.exit(result.status ?? 1);
  }
  return result.stdout;
}

const defaultTree = desktopDependencyTree();
const lightweightTree = desktopDependencyTree(["--no-default-features"]);

const reqwestVersions = new Set();
for (const line of defaultTree.split("\n")) {
  const match = line.trim().match(/^reqwest v([^\s]+)/);
  if (match) reqwestVersions.add(match[1]);
}

function dependencyVersions(name) {
  const versions = new Set();
  const prefix = `${name} v`;
  for (const line of defaultTree.split("\n")) {
    const dependency = line.trim();
    if (dependency.startsWith(prefix)) {
      versions.add(dependency.slice(prefix.length).split(/\s/, 1)[0]);
    }
  }
  return versions;
}

function versionAtLeast(version, minimum) {
  const current = version.split(".").map(Number);
  const required = minimum.split(".").map(Number);
  for (let index = 0; index < Math.max(current.length, required.length); index += 1) {
    const left = current[index] ?? 0;
    const right = required[index] ?? 0;
    if (left !== right) return left > right;
  }
  return true;
}

if (!defaultTree.includes("lancedb v")) {
  throw new Error("macOS desktop default features must retain the production LanceDB store");
}

for (const forbidden of ["lancedb v", "lance v", "datafusion v", "arrow v"]) {
  if (lightweightTree.includes(forbidden)) {
    throw new Error(
      `macOS desktop no-default-features tree must exclude heavy vector storage dependency ${forbidden.trim()}`
    );
  }
}

if (reqwestVersions.size !== 1 || ![...reqwestVersions][0].startsWith("0.12.")) {
  throw new Error(
    `macOS desktop must compile one reqwest 0.12.x stack; found ${
      [...reqwestVersions].join(", ") || "none"
    }`
  );
}

for (const forbidden of ["aws-lc-rs v", "aws-lc-sys v"]) {
  if (defaultTree.includes(forbidden)) {
    throw new Error(`macOS desktop dependency tree contains forbidden ${forbidden.trim()}`);
  }
}

for (const [name, minimum] of [
  ["event-listener", "5.4.2"],
  ["quick-xml", "0.41.0"],
]) {
  const versions = dependencyVersions(name);
  const vulnerable = [...versions].filter((version) => !versionAtLeast(version, minimum));
  if (vulnerable.length > 0) {
    throw new Error(
      `macOS desktop dependency tree contains vulnerable ${name} versions: ${vulnerable.join(", ")}`
    );
  }
}

console.log(
  `Desktop dependency policy passed (${target}, reqwest ${
    [...reqwestVersions][0]
  }, ring TLS, patched XML and event listener stacks, explicit lightweight vector-store boundary).`
);
