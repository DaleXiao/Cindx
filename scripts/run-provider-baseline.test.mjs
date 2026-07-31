import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  GPQA_SHA256,
  analyzerInvocation,
  cargoInvocation,
  parseArguments,
  providerEnvironment,
  validateOutputPaths,
  validatePostflightFacts,
  validatePreflightFacts
} from "./run-provider-baseline.mjs";

const root = path.resolve("/tmp/cindx-provider-baseline-repo");

test("execution is explicit and all paths are required", () => {
  const options = parseArguments([
    "--gpqa", "/private/gpqa.csv",
    "--raw", "/private/raw.json",
    "--sanitized", "/private/report.json",
    "--markdown", "/private/report.md"
  ]);
  assert.equal(options.execute, false);
  assert.equal(parseArguments(["--execute", ...[
    "--gpqa", "/private/gpqa.csv",
    "--raw", "/private/raw.json",
    "--sanitized", "/private/report.json",
    "--markdown", "/private/report.md"
  ]]).execute, true);
  assert.throws(() => parseArguments(["--execute"]), /--gpqa is required/);
  assert.throws(() => parseArguments(["--unknown"]), /unknown argument/);
});

test("raw stays outside while publishable reports may use docs evaluations", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-provider-paths-"));
  const repository = path.join(directory, "repo");
  const outside = path.join(directory, "outside");
  fs.mkdirSync(path.join(repository, "docs", "evaluations"), { recursive: true });
  fs.mkdirSync(path.join(repository, "target"));
  fs.mkdirSync(outside);
  try {
    const accepted = validateOutputPaths(repository, {
      raw: path.join(outside, "raw.json"),
      sanitized: path.join(repository, "docs/evaluations/current.json"),
      markdown: path.join(repository, "docs/evaluations/current.md")
    });
    assert.equal(
      accepted.raw,
      path.join(fs.realpathSync.native(outside), "raw.json")
    );
    assert.throws(
      () => validateOutputPaths(repository, {
        raw: path.join(repository, "target/raw.json"),
        sanitized: path.join(outside, "report.json"),
        markdown: path.join(outside, "report.md")
      }),
      /--raw must be outside/
    );
    assert.throws(
      () => validateOutputPaths(repository, {
        raw: path.join(outside, "raw.json"),
        sanitized: path.join(repository, "target/report.json"),
        markdown: path.join(outside, "report.md")
      }),
      /sanitized.*docs\/evaluations/
    );

    const linkedParent = path.join(outside, "linked-parent");
    fs.symlinkSync(path.join(repository, "docs", "evaluations"), linkedParent, "dir");
    assert.throws(
      () => validateOutputPaths(repository, {
        raw: path.join(linkedParent, "raw.json"),
        sanitized: path.join(repository, "docs/evaluations/report.json"),
        markdown: path.join(repository, "docs/evaluations/report.md")
      }),
      /--raw must be outside/
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("preflight rejects dataset drift, dirty tracked files, and abbreviated heads", () => {
  assert.deepEqual(
    validatePreflightFacts({
      gpqaSha256: GPQA_SHA256,
      gitHead: "a".repeat(40),
      trackedStatus: "",
      appVersion: "0.1.78",
      expectedAppVersion: "0.1.78"
    }),
    { gpqaSha256: GPQA_SHA256, gitHead: "a".repeat(40) }
  );
  assert.throws(() => validatePreflightFacts({
    gpqaSha256: "b".repeat(64), gitHead: "a".repeat(40), trackedStatus: "",
    appVersion: "0.1.78", expectedAppVersion: "0.1.78"
  }), /SHA-256 mismatch/);
  assert.throws(() => validatePreflightFacts({
    gpqaSha256: GPQA_SHA256, gitHead: "a".repeat(39), trackedStatus: "",
    appVersion: "0.1.78", expectedAppVersion: "0.1.78"
  }), /full lowercase/);
  assert.throws(() => validatePreflightFacts({
    gpqaSha256: GPQA_SHA256, gitHead: "a".repeat(40), trackedStatus: " M tracked.rs",
    appVersion: "0.1.78", expectedAppVersion: "0.1.78"
  }), /tracked worktree changes/);
  assert.throws(() => validatePreflightFacts({
    gpqaSha256: GPQA_SHA256, gitHead: "a".repeat(40), trackedStatus: "",
    appVersion: "0.1.79", expectedAppVersion: "0.1.78"
  }), /does not match baseline/);
});

test("postflight binds raw evidence to an unchanged committed HEAD", () => {
  validatePostflightFacts({
    initialHead: "a".repeat(40),
    finalHead: "a".repeat(40),
    trackedStatus: "",
    rawGitCommit: "a".repeat(40)
  });
  assert.throws(() => validatePostflightFacts({
    initialHead: "a".repeat(40), finalHead: "b".repeat(40),
    trackedStatus: "", rawGitCommit: "a".repeat(40)
  }), /Git commit changed/);
  assert.throws(() => validatePostflightFacts({
    initialHead: "a".repeat(40), finalHead: "a".repeat(40),
    trackedStatus: " M source.rs", rawGitCommit: "a".repeat(40)
  }), /worktree changed/);
});

test("provider environment pins the matrix without inspecting credentials", () => {
  const environment = providerEnvironment(
    {
      SAFE_VALUE: "retained",
      CINDX_GPQA_CASE_LIMIT: "1",
      CINDX_EVAL_FROZEN_GEPA_AUTO_PATH: "/private/auto.json",
      CINDX_EVAL_FROZEN_GEPA_PRO_PATH: "/private/pro.json"
    },
    { gitHead: "a".repeat(40), gpqa: "/private/gpqa.csv", raw: "/private/raw.json" }
  );
  assert.equal(environment.SAFE_VALUE, "retained");
  assert.equal(environment.CINDX_PROVIDER_BASELINE, "1");
  assert.equal(environment.CINDX_GPQA_PER_DOMAIN, "4");
  assert.equal(environment.CINDX_MRCR_JSONS, "");
  assert.equal(environment.CINDX_GPQA_CASE_LIMIT, undefined);
  assert.equal(environment.CINDX_EVAL_FROZEN_GEPA_AUTO_PATH, undefined);
  assert.equal(environment.CINDX_EVAL_FROZEN_GEPA_PRO_PATH, undefined);
});

test("provider and analyzer invocations use argv and the frozen contract", () => {
  const cargo = cargoInvocation(root);
  assert.equal(cargo.command, "cargo");
  assert.equal(
    cargo.args[cargo.args.indexOf("--locked") + 1],
    "external_effect_eval_tests::provider_backed_fugu_external_effect_pilot"
  );
  assert.ok(!cargo.args.includes("-c"));

  const analyzer = analyzerInvocation(root, {
    raw: "/private/raw.json",
    sanitized: "/private/report.json",
    markdown: "/private/report.md"
  });
  assert.equal(analyzer.command, "python3");
  assert.ok(analyzer.args.includes("--baseline-contract"));
  assert.ok(analyzer.args.some((value) => value.endsWith("provider-baseline-v1.json")));
});
