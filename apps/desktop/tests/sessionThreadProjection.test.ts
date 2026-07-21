import assert from "node:assert/strict";
import test from "node:test";
import {
  associateOutputArtifacts,
  buildSessionThreadProjection,
  sessionMinimapMarkers,
  updateSessionThreadProjection
} from "../src/components/sessionThreadProjection.ts";

function message(
  sequence: number,
  role: "user" | "assistant" | "tool" | "reviewer",
  content: string,
  timestampMs: number,
  runId: string | null = null
) {
  return { sequence, role, content, timestampMs, runId };
}

function event(
  sequence: number,
  label: string,
  kind: "message" | "tool" | "permission" | "model",
  timestampMs: number
) {
  return { sequence, label, detail: label, kind, state: "done" as const, timestampMs };
}

function artifact(id: string, timestampMs: number, runId: string | null = null) {
  return {
    id,
    path: `/tmp/${id}`,
    sourcePath: null,
    toolName: "file.write",
    status: "done",
    timestampMs,
    runId,
    version: 1,
    kind: "file" as const
  };
}

test("projects messages and visible events into stable rows and minimap markers", () => {
  const projection = buildSessionThreadProjection(
    [
      message(1, "user", "Build it", 100),
      message(4, "tool", "tool=shell.run", 350),
      message(5, "assistant", "Tool request", 360),
      message(6, "assistant", "Done", 400, "run-a")
    ],
    [
      event(1, "Message", "message", 100),
      event(2, "Model started", "model", 200),
      event(3, "Tool proposed", "tool", 300),
      event(7, "Model finished", "model", 410)
    ],
    32
  );

  assert.deepEqual(
    projection.items.map((item) => item.id),
    ["message-1", "event-3", "message-4", "message-5", "message-6"]
  );
  assert.equal(projection.rows.length, 3);
  assert.equal(projection.rows[1].type, "tool-chain");
  assert.equal(projection.rowIndexByItemId.get("event-3"), 1);
  assert.equal(projection.rowIndexByItemId.get("message-5"), 1);
  assert.deepEqual(
    projection.minimapMarkers.map((marker) => marker.id),
    ["message-1", "message-6"]
  );
});

test("associates artifacts without rescanning assistants for every artifact", () => {
  const projection = buildSessionThreadProjection(
    [
      message(1, "user", "Start", 100),
      message(2, "assistant", "First", 400, "run-a"),
      message(3, "assistant", "Second", 600, "run-a"),
      message(4, "user", "Follow up", 700)
    ],
    [],
    32
  );
  const result = associateOutputArtifacts(projection, [
    artifact("by-time", 350),
    artifact("by-run", 450, "run-a"),
    artifact("old-unmatched", 650),
    artifact("trailing", 800)
  ]);

  assert.deepEqual(
    result.artifactsByMessageId.get("message-2")?.map((item) => item.id),
    ["by-time"]
  );
  assert.deepEqual(
    result.artifactsByMessageId.get("message-3")?.map((item) => item.id),
    ["by-run"]
  );
  assert.deepEqual(result.trailingArtifacts.map((item) => item.id), ["trailing"]);
});

test("keeps minimap sampling bounded when a streaming answer is present", () => {
  const messages = Array.from({ length: 80 }, (_, index) =>
    message(index + 1, index % 2 === 0 ? "user" : "assistant", `Message ${index}`, index)
  );
  const projection = buildSessionThreadProjection(messages, [], 32);
  const markers = sessionMinimapMarkers(projection, "Streaming", 32);

  assert.equal(projection.minimapMarkers.length, 32);
  assert.equal(markers.length, 32);
  assert.equal(markers.at(-1)?.id, "streaming-answer");
});

function projectionView(projection: ReturnType<typeof buildSessionThreadProjection>) {
  return {
    items: projection.items.map((item) => item.id),
    rows: projection.rows.map((row) =>
      row.type === "item" ? [row.type, row.item.id] : [row.type, ...row.items.map((item) => item.id)]
    ),
    rowIndexes: [...projection.rowIndexByItemId.entries()],
    markers: projection.minimapMarkers.map((marker) => [marker.id, marker.targetIndex]),
    assistants: projection.assistants.map((item) => item.id)
  };
}

test("append-only projection matches a full rebuild across an activity boundary", () => {
  const firstMessage = message(1, "user", "Start", 100);
  const firstEvent = event(2, "Tool proposed", "tool", 200);
  const initialMessages = [firstMessage];
  const initialTimeline = [firstEvent];
  const initial = buildSessionThreadProjection(initialMessages, initialTimeline, 32);
  const nextMessages = [
    firstMessage,
    message(3, "tool", "tool=shell.run", 300),
    message(4, "assistant", "Tool request", 400),
    message(5, "assistant", "Finished", 500, "run-a")
  ];
  const hiddenModelEvent = event(6, "Model finished", "model", 600);
  const nextTimeline = [firstEvent, hiddenModelEvent];
  const incremental = updateSessionThreadProjection(
    initial,
    nextMessages,
    nextTimeline,
    32
  );
  const rebuilt = buildSessionThreadProjection(nextMessages, nextTimeline, 32);

  assert.deepEqual(projectionView(incremental), projectionView(rebuilt));
  assert.equal(incremental.visibleTimelineItemCount, rebuilt.visibleTimelineItemCount);
});

test("projection falls back safely when history is prepended", () => {
  const existing = message(2, "assistant", "Existing", 200);
  const initial = buildSessionThreadProjection([existing], [], 32);
  const messages = [message(1, "user", "Earlier", 100), existing];
  const updated = updateSessionThreadProjection(initial, messages, [], 32);
  const rebuilt = buildSessionThreadProjection(messages, [], 32);

  assert.deepEqual(projectionView(updated), projectionView(rebuilt));
});
