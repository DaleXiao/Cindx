import assert from "node:assert/strict";
import test from "node:test";
import { activeRunProgress } from "../src/components/sessionRunProgressModel.ts";

const status = (detail: string, timestampMs: number) =>
  ({ label: "Status", detail, kind: "message", state: "done", timestampMs }) as any;

test("tracks multiple subagents and counts finished ones", () => {
  const timeline = [
    status("Agent task started", 1000),
    status("Subagent started: repo A", 1100),
    status("Subagent finished: repo A", 1150),
    status("Subagent started: repo B", 1200)
  ];

  const progress = activeRunProgress(timeline, 1000);

  assert.equal(progress.subagents.length, 2);
  assert.deepEqual(progress.subagents[0], { description: "repo A", done: true });
  assert.deepEqual(progress.subagents[1], { description: "repo B", done: false });
  assert.equal(progress.label, "Running subagents 1/2");
});

test("a single running subagent keeps the singular label", () => {
  const timeline = [
    status("Agent task started", 1000),
    status("Subagent started: deep survey", 1100)
  ];

  const progress = activeRunProgress(timeline, 1000);

  assert.equal(progress.subagents.length, 1);
  assert.equal(progress.subagents[0].done, false);
  assert.equal(progress.label, "Running subagent");
});

test("subagent finished reports done and a finished label", () => {
  const timeline = [
    status("Agent task started", 1000),
    status("Subagent started: deep survey", 1100),
    status("Subagent finished: deep survey", 1200)
  ];

  const progress = activeRunProgress(timeline, 1000);

  assert.equal(progress.subagents[0].done, true);
  assert.equal(progress.label, "Subagent done");
});
