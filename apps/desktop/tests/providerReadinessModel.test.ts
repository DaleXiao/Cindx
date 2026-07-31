import assert from "node:assert/strict";
import test from "node:test";
import {
  providerSubmissionPreflight,
  resolveProviderReadiness
} from "../src/providerReadinessModel.ts";

test("provider readiness prefers authoritative phase state and falls back to runtime state", () => {
  assert.equal(resolveProviderReadiness(null, null, false), "checking");
  assert.equal(resolveProviderReadiness(null, null, true), "error");
  assert.equal(resolveProviderReadiness(null, false, false), "unconfigured");
  assert.equal(resolveProviderReadiness(null, true, false), "ready");
  assert.equal(resolveProviderReadiness({ ready: false }, true, false), "unconfigured");
  assert.equal(resolveProviderReadiness({ ready: true }, false, false), "ready");
});

test("legacy verified-at-null providers remain runnable when Rust marks them ready", () => {
  const legacyProvider = {
    ready: true,
    apiKeySet: true,
    authVerified: false,
    authVerifiedAtMs: null
  };

  assert.equal(resolveProviderReadiness(legacyProvider, false, false), "ready");
});

test("provider preflight preserves the draft and exposes recovery only when actionable", () => {
  assert.deepEqual(providerSubmissionPreflight("checking"), {
    allowSubmit: false,
    clearDraft: false,
    showModelsCta: false
  });
  for (const readiness of ["unconfigured", "error"] as const) {
    assert.deepEqual(providerSubmissionPreflight(readiness), {
      allowSubmit: false,
      clearDraft: false,
      showModelsCta: true
    });
  }
  assert.deepEqual(providerSubmissionPreflight("ready"), {
    allowSubmit: true,
    clearDraft: true,
    showModelsCta: false
  });
});
