import assert from "node:assert/strict";
import test from "node:test";
import { composerTextareaSizing } from "../src/composerSizingModel.ts";

const MIN = 58;
const MAX = 180;

test("short content clamps to the minimum height with hidden overflow", () => {
  const sizing = composerTextareaSizing(20, MIN, MAX);
  assert.equal(sizing.heightPx, MIN);
  assert.equal(sizing.overflowY, "hidden");
});

test("mid-range content uses the scroll height with hidden overflow", () => {
  const sizing = composerTextareaSizing(120, MIN, MAX);
  assert.equal(sizing.heightPx, 120);
  assert.equal(sizing.overflowY, "hidden");
});

test("content at the maximum height stays hidden", () => {
  const sizing = composerTextareaSizing(MAX, MIN, MAX);
  assert.equal(sizing.heightPx, MAX);
  assert.equal(sizing.overflowY, "hidden");
});

test("content beyond the maximum clamps and switches overflow to auto", () => {
  const sizing = composerTextareaSizing(400, MIN, MAX);
  assert.equal(sizing.heightPx, MAX);
  assert.equal(sizing.overflowY, "auto");
});

test("zero and negative scroll heights clamp to the minimum", () => {
  assert.equal(composerTextareaSizing(0, MIN, MAX).heightPx, MIN);
  assert.equal(composerTextareaSizing(-50, MIN, MAX).heightPx, MIN);
  assert.equal(composerTextareaSizing(-50, MIN, MAX).overflowY, "hidden");
});
