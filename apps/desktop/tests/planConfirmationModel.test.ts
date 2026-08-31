import assert from "node:assert/strict";
import test from "node:test";
import { parsePlanDocument } from "../src/planConfirmationModel.ts";

const STRUCTURED = `Objective: Unify the composer dropdown radius.
Steps:
1. Centralize the radius token - move the value into foundation tokens (files: src/styles/foundation.css)
2. Apply the token to dropdowns and forms (files: src/styles/composer.css, src/styles/settings.css)
3. Verify visually - run the app and check both themes
Verification: structure gate plus a manual light/dark pass.`;

test("a structured plan parses into objective, steps, files, and verification", () => {
  const doc = parsePlanDocument(STRUCTURED);
  assert.equal(doc.structured, true);
  assert.equal(doc.objective, "Unify the composer dropdown radius.");
  assert.equal(doc.verification, "structure gate plus a manual light/dark pass.");
  assert.equal(doc.steps.length, 3);
  assert.equal(doc.steps[0].title, "Centralize the radius token");
  assert.equal(doc.steps[0].detail, "move the value into foundation tokens");
  assert.deepEqual(doc.steps[0].files, ["src/styles/foundation.css"]);
  assert.deepEqual(doc.steps[1].files, ["src/styles/composer.css", "src/styles/settings.css"]);
  assert.equal(doc.steps[2].title, "Verify visually");
  assert.equal(doc.steps[2].detail, "run the app and check both themes");
  assert.deepEqual(doc.steps[2].files, []);
});

test("chinese labels and full-width punctuation parse the same shape", () => {
  const doc = parsePlanDocument(
    "目标: 统一圆角。\n步骤:\n1. 抽取 token（文件: src/styles/foundation.css）\n验证: 跑门禁。"
  );
  assert.equal(doc.structured, true);
  assert.equal(doc.objective, "统一圆角。");
  assert.deepEqual(doc.steps[0].files, ["src/styles/foundation.css"]);
  assert.equal(doc.verification, "跑门禁。");
});

test("free-form markdown falls back to unstructured rendering", () => {
  const doc = parsePlanDocument("Here is a plan.\n- do the thing\n- then the other thing");
  assert.equal(doc.structured, false);
  assert.equal(doc.steps.length, 0);
});

test("an empty plan is unstructured", () => {
  const doc = parsePlanDocument("");
  assert.equal(doc.structured, false);
  assert.equal(doc.objective, null);
  assert.equal(doc.verification, null);
});
