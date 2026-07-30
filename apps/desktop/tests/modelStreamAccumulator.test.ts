import assert from "node:assert/strict";
import test from "node:test";
import {
  ModelStreamAccumulator
} from "../src/components/modelStreamAccumulator.ts";
import {
  MODEL_STREAM_RETIRED_REQUEST_LIMIT,
  ModelStreamEventRouter
} from "../src/components/modelStreamEventRouter.ts";
import {
  StreamingMarkdownModel,
  type StreamingMarkdownSnapshot,
  type StreamingMarkdownTailNode
} from "../src/components/streamingMarkdownModel.ts";
import type { ModelStreamDelta } from "../src/tauriTypes.ts";

class FakeScheduler {
  private nextId = 1;
  readonly tasks = new Map<number, { callback: () => void; cancelled: boolean; delayMs: number }>();
  readonly delays: number[] = [];

  schedule = (callback: () => void, delayMs: number) => {
    const id = this.nextId;
    this.nextId += 1;
    this.tasks.set(id, { callback, cancelled: false, delayMs });
    this.delays.push(delayMs);
    return id;
  };

  cancel = (id: number) => {
    const task = this.tasks.get(id);
    if (task) task.cancelled = true;
  };

  latestId() {
    return this.nextId - 1;
  }

  run(id: number, evenIfCancelled = false) {
    const task = this.tasks.get(id);
    assert.ok(task, `scheduled task ${id} should exist`);
    if (!task.cancelled || evenIfCancelled) task.callback();
  }
}

function deferredText(root: StreamingMarkdownTailNode) {
  const parts: string[] = [];
  const stack = [root];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node) continue;
    if (node.kind === "leaf") parts.push(node.content);
    else {
      for (let index = node.children.length - 1; index >= 0; index -= 1) {
        stack.push(node.children[index]);
      }
    }
  }
  return parts.join("");
}

function snapshotText(snapshot: StreamingMarkdownSnapshot) {
  return [
    ...snapshot.settledChunks.map((chunk) => chunk.content),
    snapshot.parsedTail.content,
    deferredText(snapshot.deferredTailRoot),
    snapshot.deferredTailOpen.content
  ].join("");
}

function fixture() {
  const scheduler = new FakeScheduler();
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = new ModelStreamAccumulator({
    model: new StreamingMarkdownModel(),
    schedule: scheduler.schedule,
    cancel: scheduler.cancel,
    onSnapshot: (snapshot) => snapshots.push(snapshot)
  });
  return { accumulator, scheduler, snapshots };
}

test("shows the first delta immediately and coalesces a long burst in order", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.append("first");
  for (let index = 0; index < 10_000; index += 1) accumulator.append(String(index % 10));

  assert.equal(snapshots.length, 1);
  assert.equal(snapshotText(snapshots[0]), "first");
  assert.deepEqual(scheduler.delays, [80, 16]);

  scheduler.run(scheduler.latestId());
  assert.equal(snapshots.length, 2);
  assert.equal(
    snapshotText(snapshots[1]),
    `first${Array.from({ length: 10_000 }, (_, index) => index % 10).join("")}`
  );
});

test("adapts the normal drain interval as accumulated output grows", () => {
  const medium = fixture();
  medium.accumulator.append("a".repeat(16 * 1024));
  medium.accumulator.append("b");
  assert.equal(medium.scheduler.delays.at(-1), 100);

  const long = fixture();
  long.accumulator.append("a".repeat(64 * 1024));
  long.accumulator.append("b");
  assert.equal(long.scheduler.delays.at(-1), 120);
});

test("hidden streams only aggregate and visibility restoration drains once", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.setVisible(false);
  accumulator.append("hidden-");
  accumulator.append("answer");
  assert.equal(snapshots.length, 0);
  assert.equal(scheduler.delays.length, 0);

  accumulator.setVisible(true);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshotText(snapshots[0]), "hidden-answer");

  accumulator.append("-pending");
  const staleId = scheduler.latestId();
  accumulator.setVisible(false);
  scheduler.run(staleId, true);
  assert.equal(snapshots.length, 1);
  accumulator.append("-more");
  accumulator.setVisible(true);
  assert.equal(snapshots.length, 2);
  assert.equal(snapshotText(snapshots[1]), "hidden-answer-pending-more");
});

test("hidden token bursts transfer bounded slabs without publishing", () => {
  const { accumulator, snapshots } = fixture();
  const length = 8 * 1024 * 16;
  accumulator.setVisible(false);
  for (let index = 0; index < length; index += 1) accumulator.append("x");

  assert.equal(accumulator.bufferedFragmentCount, 0);
  assert.equal(accumulator.bufferedCharacterCount, 0);
  assert.equal(snapshots.length, 0);
  assert.equal(snapshotText(accumulator.snapshot()), "x".repeat(length));
  accumulator.finish();
  assert.equal(accumulator.bufferedFragmentCount, 0);
  assert.equal(snapshots.length, 0);
  assert.equal(snapshotText(accumulator.snapshot()), "x".repeat(length));

  accumulator.setVisible(true);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshotText(snapshots[0]), "x".repeat(length));
});

test("hidden finish transfers the small terminal tail without publishing", () => {
  const { accumulator, snapshots } = fixture();
  accumulator.setVisible(false);
  accumulator.append("hidden terminal tail");

  accumulator.finish();
  assert.equal(accumulator.bufferedCharacterCount, 0);
  assert.equal(snapshotText(accumulator.snapshot()), "hidden terminal tail");
  assert.equal(snapshots.length, 0);

  accumulator.setVisible(true);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshotText(snapshots[0]), "hidden terminal tail");
});

test("revealing a hidden reset keeps the next first token immediate", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.setVisible(false);
  accumulator.reset();
  accumulator.setVisible(true);
  assert.equal(snapshotText(snapshots.at(-1)!), "");

  const publishedBeforeDelta = snapshots.length;
  accumulator.append("first");
  assert.equal(snapshots.length, publishedBeforeDelta + 1);
  assert.equal(snapshotText(snapshots.at(-1)!), "first");
  assert.equal(scheduler.delays.length, 0);
});

test("finish synchronously drains the tail and invalidates the old callback", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.append("a");
  accumulator.append("b");
  const staleId = scheduler.latestId();

  accumulator.finish();
  assert.equal(snapshotText(snapshots.at(-1)!), "ab");
  assert.equal(snapshots.length, 2);
  scheduler.run(staleId, true);
  assert.equal(snapshots.length, 2);
});

test("reset clears pending text and makes cancelled callbacks stale", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.append("old");
  accumulator.append("-pending");
  const staleId = scheduler.latestId();

  accumulator.reset();
  assert.equal(snapshotText(snapshots.at(-1)!), "");
  scheduler.run(staleId, true);
  assert.equal(snapshotText(snapshots.at(-1)!), "");
  accumulator.append("new");
  assert.equal(snapshotText(snapshots.at(-1)!), "new");
});

test("dispose cancels pending work without publishing cleanup state", () => {
  const { accumulator, scheduler, snapshots } = fixture();
  accumulator.append("visible");
  accumulator.append("-pending");
  const staleId = scheduler.latestId();
  const publishedBeforeDispose = snapshots.length;

  accumulator.dispose();
  scheduler.run(staleId, true);
  accumulator.append("ignored");
  accumulator.finish();
  accumulator.reset();
  assert.equal(snapshots.length, publishedBeforeDispose);
});

function event(overrides: Partial<ModelStreamDelta> = {}): ModelStreamDelta {
  return {
    taskId: "phase-16-agent-loop",
    requestId: "request-1",
    sessionId: "session-1",
    delta: "delta",
    done: false,
    reset: false,
    error: null,
    ...overrides
  };
}

test("routes only exact non-null agent-loop session events", () => {
  const router = new ModelStreamEventRouter();
  assert.deepEqual(router.route(event(), "session-1"), {
    kind: "apply",
    requestId: "request-1",
    reset: false,
    delta: "delta",
    done: false,
    error: null
  });
  assert.deepEqual(
    router.route(
      event({ taskId: "phase-4-model-settings", requestId: "settings", reset: true }),
      "session-1"
    ),
    { kind: "ignore" }
  );
  assert.deepEqual(router.route(event({ sessionId: null }), "session-1"), {
    kind: "ignore"
  });
  assert.deepEqual(router.route(event(), null), { kind: "ignore" });
  assert.deepEqual(router.route(event(), "session-2"), { kind: "ignore" });
  assert.equal(router.activeRequest, "request-1");
  assert.equal(router.retiredRequestCount, 0);
});

test("an active empty reset opens the next request and rejects late old deltas", () => {
  const router = new ModelStreamEventRouter();
  assert.equal(router.route(event({ requestId: "old" }), "session-1").kind, "apply");
  assert.deepEqual(
    router.route(event({ requestId: "old", reset: true, delta: "" }), "session-1"),
    {
      kind: "apply",
      requestId: "old",
      reset: true,
      delta: "",
      done: false,
      error: null
    }
  );
  assert.equal(router.activeRequest, null);
  assert.equal(router.retiredRequestCount, 0);
  assert.equal(router.route(event({ requestId: "new", delta: "new answer" }), "session-1").kind, "apply");
  assert.equal(router.retiredRequestCount, 1);
  assert.deepEqual(router.route(event({ requestId: "old", delta: "late" }), "session-1"), {
    kind: "ignore"
  });
  assert.equal(router.route(event({ requestId: "new", delta: " continues" }), "session-1").kind, "apply");
});

test("a reset carrying content directly takes over the stream", () => {
  const router = new ModelStreamEventRouter();
  router.route(event({ requestId: "old" }), "session-1");
  const takeover = router.route(
    event({ requestId: "new", reset: true, delta: "replacement" }),
    "session-1"
  );
  assert.deepEqual(takeover, {
    kind: "apply",
    requestId: "new",
    reset: true,
    delta: "replacement",
    done: false,
    error: null
  });
  assert.deepEqual(router.route(event({ requestId: "old" }), "session-1"), { kind: "ignore" });
  assert.equal(router.route(event({ requestId: "new", delta: " next" }), "session-1").kind, "apply");
});

test("foreign empty terminal events refresh without joining stream content", () => {
  const router = new ModelStreamEventRouter();
  router.route(event(), "session-1");
  assert.deepEqual(
    router.route(
      event({ requestId: "outer-run", delta: "", done: true, error: "stopped" }),
      "session-1"
    ),
    { kind: "terminal-refresh", error: "stopped", settlesStream: true }
  );
  assert.equal(router.route(event({ delta: "still active" }), "session-1").kind, "apply");
  assert.deepEqual(
    router.route(event({ delta: "", done: true }), "session-1"),
    {
      kind: "apply",
      requestId: "request-1",
      reset: false,
      delta: "",
      done: true,
      error: null
    }
  );
  assert.deepEqual(router.route(event({ delta: "late" }), "session-1"), { kind: "ignore" });
});

test("a new request after a foreign terminal starts a clean stream generation", () => {
  const router = new ModelStreamEventRouter();
  router.route(event({ requestId: "synthesis", delta: "answer" }), "session-1");
  assert.deepEqual(
    router.route(event({ requestId: "outer-run", delta: "", done: true }), "session-1"),
    { kind: "terminal-refresh", error: null, settlesStream: true }
  );
  assert.deepEqual(
    router.route(event({ requestId: "next-run", delta: "next" }), "session-1"),
    {
      kind: "apply",
      requestId: "next-run",
      reset: true,
      delta: "next",
      done: false,
      error: null
    }
  );
  assert.deepEqual(router.route(event({ requestId: "synthesis", delta: "late" }), "session-1"), {
    kind: "ignore"
  });
});

test("a duplicate retired terminal refreshes state without settling a newer stream", () => {
  const router = new ModelStreamEventRouter();
  router.route(event({ requestId: "old", delta: "old" }), "session-1");
  router.route(event({ requestId: "old", delta: "", done: true }), "session-1");
  router.route(event({ requestId: "new", delta: "new" }), "session-1");

  assert.deepEqual(
    router.route(event({ requestId: "old", delta: "", done: true }), "session-1"),
    { kind: "terminal-refresh", error: null, settlesStream: false }
  );
  assert.equal(router.route(event({ requestId: "new", delta: " continues" }), "session-1").kind, "apply");
});

test("a fallback reset can replace an active synthesis request", () => {
  const router = new ModelStreamEventRouter();
  router.route(event({ requestId: "synthesis" }), "session-1");
  assert.deepEqual(
    router.route(event({ requestId: "original", reset: true, delta: "" }), "session-1"),
    {
      kind: "apply",
      requestId: "original",
      reset: true,
      delta: "",
      done: false,
      error: null
    }
  );
  assert.equal(
    router.route(event({ requestId: "original", delta: "fallback answer" }), "session-1").kind,
    "apply"
  );
  assert.deepEqual(router.route(event({ requestId: "synthesis", delta: "late" }), "session-1"), {
    kind: "ignore"
  });
});

test("an empty reset allows the same request id to claim a new generation", () => {
  const router = new ModelStreamEventRouter();
  router.route(event({ requestId: "request-X", reset: true, delta: "" }), "session-1");
  assert.equal(
    router.route(event({ requestId: "request-X", delta: "fallback answer" }), "session-1").kind,
    "apply"
  );
  assert.equal(router.activeRequest, "request-X");
});

test("retired request tracking stays bounded and resets with the session lifecycle", () => {
  const router = new ModelStreamEventRouter();
  for (let index = 0; index < MODEL_STREAM_RETIRED_REQUEST_LIMIT + 5; index += 1) {
    const requestId = `request-${index}`;
    assert.equal(router.route(event({ requestId }), "session-1").kind, "apply");
    assert.equal(
      router.route(event({ requestId, reset: true, delta: "" }), "session-1").kind,
      "apply"
    );
  }
  assert.equal(router.activeRequest, null);
  assert.equal(router.retiredRequestCount, MODEL_STREAM_RETIRED_REQUEST_LIMIT);

  router.reset();
  assert.equal(router.activeRequest, null);
  assert.equal(router.retiredRequestCount, 0);
  assert.equal(router.route(event({ requestId: "request-12" }), "session-1").kind, "apply");
});

test("same-session preparation retires old identities without forgetting them", () => {
  const router = new ModelStreamEventRouter();
  assert.equal(router.route(event({ requestId: "old" }), "session-1").kind, "apply");

  router.prepareForNextRequest();
  assert.equal(router.activeRequest, null);
  assert.equal(router.retiredRequestCount, 1);
  assert.deepEqual(router.route(event({ requestId: "old", delta: "late" }), "session-1"), {
    kind: "ignore"
  });
  assert.deepEqual(
    router.route(event({ requestId: "old", reset: true, delta: "" }), "session-1"),
    { kind: "ignore" }
  );
  assert.equal(router.route(event({ requestId: "new", delta: "new" }), "session-1").kind, "apply");
});
