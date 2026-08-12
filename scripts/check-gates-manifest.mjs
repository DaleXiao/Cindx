#!/usr/bin/env node
// Fail-fast integrity check for benchmark contracts and the quality-gates
// manifest. Runs before any cargo or npm step so a merge-corrupted JSON file
// breaks the pipeline in seconds instead of after a full toolchain warmup.
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readJson(relativePath) {
  const fullPath = path.join(root, relativePath);
  let text;
  try {
    text = fs.readFileSync(fullPath, "utf8");
  } catch (error) {
    throw new Error(`${relativePath}: unreadable (${error.message})`);
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`${relativePath}: invalid JSON (${error.message})`);
  }
}

function collectBenchmarkJson(directory, accumulator = []) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      collectBenchmarkJson(entryPath, accumulator);
    } else if (entry.name.endsWith(".json")) {
      accumulator.push(path.relative(root, entryPath));
    }
  }
  return accumulator;
}

const failures = [];

// 1. Every tracked benchmark contract must parse.
for (const relativePath of collectBenchmarkJson(path.join(root, "benchmarks")).sort()) {
  if (relativePath === path.join("benchmarks", "system", "quality-gates-v1.json")) {
    continue; // validated more deeply below
  }
  try {
    readJson(relativePath);
  } catch (error) {
    failures.push(error.message);
  }
}

// 2. The quality-gates manifest must be structurally consistent.
const manifestPath = path.join("benchmarks", "system", "quality-gates-v1.json");
let manifest = null;
try {
  manifest = readJson(manifestPath);
} catch (error) {
  failures.push(error.message);
}

if (manifest) {
  if (manifest.schema !== "cindx.quality-gates.v1") {
    failures.push(`${manifestPath}: unexpected schema "${manifest.schema}"`);
  }
  const gates = Array.isArray(manifest.gates) ? manifest.gates : [];
  if (gates.length === 0) {
    failures.push(`${manifestPath}: no gates defined`);
  }
  const gateIds = new Set();
  gates.forEach((gate, index) => {
    if (typeof gate?.id !== "string" || gate.id.length === 0) {
      failures.push(`${manifestPath}: gate #${index} has no id`);
      return;
    }
    if (gateIds.has(gate.id)) {
      failures.push(`${manifestPath}: duplicate gate id "${gate.id}"`);
    }
    gateIds.add(gate.id);
    if (typeof gate.category !== "string" || gate.category.length === 0) {
      failures.push(`${manifestPath}: gate "${gate.id}" has no category`);
    }
    if (!Array.isArray(gate.command) || gate.command.length === 0) {
      failures.push(`${manifestPath}: gate "${gate.id}" has no command`);
    }
    for (const required of gate.required_output ?? []) {
      if (typeof required !== "string" || required.length === 0) {
        failures.push(`${manifestPath}: gate "${gate.id}" has a non-string required_output entry`);
      }
    }
    for (const assertion of gate.assertions ?? []) {
      if (typeof assertion?.path !== "string" || assertion.path.length === 0) {
        failures.push(`${manifestPath}: gate "${gate.id}" has an assertion without a path`);
      }
    }
  });
  const profiles = manifest.profiles ?? {};
  if (typeof profiles !== "object" || Object.keys(profiles).length === 0) {
    failures.push(`${manifestPath}: no profiles defined`);
  }
  const referenced = new Set();
  for (const [profile, gateList] of Object.entries(profiles)) {
    if (!Array.isArray(gateList) || gateList.length === 0) {
      failures.push(`${manifestPath}: profile "${profile}" is empty`);
      continue;
    }
    const seenInProfile = new Set();
    for (const gateId of gateList) {
      if (!gateIds.has(gateId)) {
        failures.push(`${manifestPath}: profile "${profile}" references unknown gate "${gateId}"`);
      }
      if (seenInProfile.has(gateId)) {
        failures.push(`${manifestPath}: profile "${profile}" lists "${gateId}" twice`);
      }
      seenInProfile.add(gateId);
      referenced.add(gateId);
    }
  }
  for (const gateId of gateIds) {
    if (!referenced.has(gateId)) {
      failures.push(`${manifestPath}: gate "${gateId}" is not referenced by any profile`);
    }
  }
  for (const required of ["quick", "ci-contract", "control-plane", "full"]) {
    if (!Array.isArray(profiles[required])) {
      failures.push(`${manifestPath}: required profile "${required}" is missing`);
    }
  }
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error(`[gates-manifest] ${failure}`);
  }
  process.exit(1);
}

const gateCount = manifest ? manifest.gates.length : 0;
const profileCount = manifest ? Object.keys(manifest.profiles ?? {}).length : 0;
console.log(
  `Gates manifest integrity ok (${gateCount} gates, ${profileCount} profiles, all benchmark JSON parsed)`
);
