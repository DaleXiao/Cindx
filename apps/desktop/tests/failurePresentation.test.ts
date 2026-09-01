import assert from "node:assert/strict";
import test from "node:test";
import { presentFailure } from "../src/failurePresentation.ts";

test("contract unsatisfied failures are translated to plain language", () => {
  const presented = presentFailure(
    "task contract requirement `prompt_tool:2:image.generate` remained unsatisfied after 2 repair attempts"
  );
  assert.ok(presented.summary.includes("image.generate"));
  assert.ok(presented.summary.includes("cancelled or redirected"));
  assert.ok(
    presented.detail?.includes("remained unsatisfied after 2 repair attempts"),
    "raw diagnostic text is retained behind the details toggle"
  );
  assert.ok(!presented.summary.includes("repair attempts"), "summary hides internal jargon");
});

test("unavailable tool failures name the tool and stay actionable", () => {
  const presented = presentFailure("task contract requires unavailable tool `web.fetch`");
  assert.ok(presented.summary.includes("web.fetch"));
  assert.ok(presented.summary.includes("isn't available"));
});

test("unknown failures pass through unchanged so nothing is hidden", () => {
  const raw = "some brand new failure the mapper does not know";
  const presented = presentFailure(raw);
  assert.equal(presented.summary, raw);
  assert.equal(presented.detail, null);
});
