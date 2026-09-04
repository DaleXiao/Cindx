import assert from "node:assert/strict";
import test from "node:test";
import { shouldRevealMainWindow } from "../src/startupRevealModel.ts";

test("the window reveals once both initial state requests succeeded", () => {
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: true,
      projectSessionReady: true,
      bootstrapFailed: false
    }),
    true
  );
});

test("the window stays hidden while the initial state is still loading", () => {
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: false,
      projectSessionReady: false,
      bootstrapFailed: false
    }),
    false
  );
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: true,
      projectSessionReady: false,
      bootstrapFailed: false
    }),
    false,
    "a half-loaded shell must not be the first paint"
  );
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: false,
      projectSessionReady: true,
      bootstrapFailed: false
    }),
    false
  );
});

test("a failed bootstrap reveals the window instead of leaving the app invisible", () => {
  // The regression this model exists for: one rejected initial-state command used
  // to keep the window hidden forever, with no error surface and no log entry.
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: false,
      projectSessionReady: false,
      bootstrapFailed: true
    }),
    true
  );
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: true,
      projectSessionReady: false,
      bootstrapFailed: true
    }),
    true
  );
});

test("a late success still reveals after a failure was recorded", () => {
  assert.equal(
    shouldRevealMainWindow({
      runtimeReady: true,
      projectSessionReady: true,
      bootstrapFailed: true
    }),
    true
  );
});
