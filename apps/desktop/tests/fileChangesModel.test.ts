import assert from "node:assert/strict";
import test from "node:test";
import {
  groupFileChangesByRun,
  lastMessageIdByRun,
  latestFileChangesRunId
} from "../src/fileChangesModel.ts";

function entry(runId: string | null, id: string) {
  return {
    toolCallId: id,
    sequence: 1,
    tool: "file.write",
    path: `${id}.ts`,
    action: "overwritten",
    undone: false,
    undoable: true,
    runId
  } as any;
}

function message(role: string, runId: string | null, index: number) {
  return { role, runId, content: "", timestampMs: index } as any;
}

test("entries group by their run and orphans join the latest run", () => {
  const groups = groupFileChangesByRun(
    [entry("run-a", "a1"), entry("run-b", "b1"), entry(null, "old")],
    "run-b"
  );
  assert.deepEqual(groups.get("run-a")?.map((e) => e.toolCallId), ["a1"]);
  assert.deepEqual(groups.get("run-b")?.map((e) => e.toolCallId), ["b1", "old"]);
});

test("the latest run is the newest attributed message", () => {
  const messages = [
    message("user", "run-a", 0),
    message("assistant", "run-a", 1),
    message("user", "run-b", 2),
    message("assistant", "run-b", 3)
  ];
  assert.equal(latestFileChangesRunId(messages), "run-b");
});

test("a trailing user message means a new turn started, so no run is latest", () => {
  const messages = [
    message("user", "run-a", 0),
    message("assistant", "run-a", 1),
    message("user", null, 2)
  ];
  assert.equal(latestFileChangesRunId(messages), null);
});

test("each run's panel attaches to the last message carrying that run", () => {
  const messages = [
    message("user", "run-a", 0),
    message("assistant", "run-a", 1),
    message("user", "run-b", 2),
    message("assistant", "run-b", 3)
  ];
  const targets = lastMessageIdByRun(messages, (m, i) => `${m.role}-${i}`);
  assert.equal(targets.get("run-a"), "assistant-1");
  assert.equal(targets.get("run-b"), "assistant-3");
});
