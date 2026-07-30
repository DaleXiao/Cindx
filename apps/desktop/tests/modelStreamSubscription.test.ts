import assert from "node:assert/strict";
import test from "node:test";
import { ModelStreamAccumulator } from "../src/components/modelStreamAccumulator.ts";
import { ModelStreamEventRouter } from "../src/components/modelStreamEventRouter.ts";
import {
  startModelStreamSubscription,
  type ModelStreamSubscribe
} from "../src/components/modelStreamSubscription.ts";
import {
  StreamingMarkdownModel,
  type StreamingMarkdownSnapshot,
  type StreamingMarkdownTailNode
} from "../src/components/streamingMarkdownModel.ts";
import type { ModelStreamDelta } from "../src/tauriTypes.ts";

type StreamListener = (payload: ModelStreamDelta) => void;

class FakeScheduler {
  private nextId = 1;
  private readonly tasks = new Map<number, () => void>();

  schedule = (callback: () => void) => {
    const id = this.nextId;
    this.nextId += 1;
    this.tasks.set(id, callback);
    return id;
  };

  cancel = (id: number) => {
    this.tasks.delete(id);
  };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function controlledSubscription() {
  const registration = deferred<() => void>();
  let listener: StreamListener | null = null;
  const subscribe: ModelStreamSubscribe = (receive) => {
    listener = receive;
    return registration.promise;
  };
  return {
    listener: () => {
      assert.ok(listener, "subscription listener should be registered synchronously");
      return listener;
    },
    registration,
    subscribe
  };
}

function accumulatorFixture(snapshots: StreamingMarkdownSnapshot[]) {
  const scheduler = new FakeScheduler();
  return new ModelStreamAccumulator({
    model: new StreamingMarkdownModel(),
    schedule: scheduler.schedule,
    cancel: scheduler.cancel,
    onSnapshot: (snapshot) => snapshots.push(snapshot)
  });
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

function event(
  sessionId: string,
  requestId: string,
  overrides: Partial<ModelStreamDelta> = {}
): ModelStreamDelta {
  return {
    taskId: "phase-16-agent-loop",
    requestId,
    sessionId,
    delta: "delta",
    done: false,
    reset: false,
    error: null,
    ...overrides
  };
}

async function flushPromiseJobs() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

test("dispose before subscribe resolves immediately unlistens and invalidates the listener", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const done: string[] = [];
  const errors: string[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  let unlistenCount = 0;

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return false;
    },
    onError: (message) => errors.push(message)
  });
  const staleListener = controlled.listener();

  dispose();
  staleListener(event("session-A", "request-old"));
  controlled.registration.resolve(() => {
    unlistenCount += 1;
  });
  await flushPromiseJobs();
  staleListener(event("session-A", "request-old", { delta: "", done: true }));

  assert.equal(unlistenCount, 1);
  assert.equal(snapshots.length, 0);
  assert.deepEqual(done, []);
  assert.deepEqual(errors, []);
  accumulator.dispose();
});

test("session reset rejects terminal and delta events from the previous session", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const done: string[] = [];
  const errors: string[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  let activeSessionId = "session-A";

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => activeSessionId,
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return false;
    },
    onError: (message) => errors.push(message)
  });
  const listener = controlled.listener();
  listener(event("session-A", "request-A", { delta: "answer-A" }));
  assert.equal(snapshotText(accumulator.snapshot()), "answer-A");

  activeSessionId = "session-B";
  router.reset();
  accumulator.reset();
  listener(event("session-A", "request-A", { delta: "", done: true }));
  listener(event("session-A", "request-A", { delta: "late-A" }));
  assert.equal(snapshotText(accumulator.snapshot()), "");
  assert.deepEqual(done, []);

  listener(event("session-B", "request-B", { delta: "answer-B" }));
  listener(event("session-B", "request-B", { delta: "", done: true }));
  assert.equal(snapshotText(accumulator.snapshot()), "answer-B");
  assert.deepEqual(done, ["session-B"]);
  assert.deepEqual(errors, []);

  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("StrictMode-style resubscription leaves the disposed listener inert", async () => {
  const router = new ModelStreamEventRouter();
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const done: string[] = [];
  const errors: string[] = [];
  const firstAccumulator = accumulatorFixture(snapshots);
  const first = controlledSubscription();
  let firstUnlistenCount = 0;

  const disposeFirst = startModelStreamSubscription({
    subscribe: first.subscribe,
    router,
    accumulator: firstAccumulator,
    activeSessionId: () => "session-A",
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return false;
    },
    onError: (message) => errors.push(message)
  });
  const staleListener = first.listener();
  disposeFirst();
  firstAccumulator.dispose();
  router.reset();

  const secondAccumulator = accumulatorFixture(snapshots);
  const second = controlledSubscription();
  let secondUnlistenCount = 0;
  const disposeSecond = startModelStreamSubscription({
    subscribe: second.subscribe,
    router,
    accumulator: secondAccumulator,
    activeSessionId: () => "session-A",
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return false;
    },
    onError: (message) => errors.push(message)
  });
  const currentListener = second.listener();

  staleListener(event("session-A", "request-old", { delta: "stale" }));
  currentListener(event("session-A", "request-new", { delta: "current" }));
  assert.equal(snapshotText(secondAccumulator.snapshot()), "current");
  assert.deepEqual(done, []);
  assert.deepEqual(errors, []);

  first.registration.resolve(() => {
    firstUnlistenCount += 1;
  });
  second.registration.resolve(() => {
    secondUnlistenCount += 1;
  });
  await flushPromiseJobs();
  assert.equal(firstUnlistenCount, 1);
  assert.equal(secondUnlistenCount, 0);

  disposeSecond();
  assert.equal(secondUnlistenCount, 1);
  staleListener(event("session-A", "request-old", { delta: "", done: true }));
  assert.deepEqual(done, []);
  secondAccumulator.dispose();
});

test("subscribe rejection reports only while the lifecycle is active", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const errors: string[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();

  const active = controlledSubscription();
  const disposeActive = startModelStreamSubscription({
    subscribe: active.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => false,
    onError: (message) => errors.push(message)
  });
  active.registration.reject(new Error("active subscribe failed"));
  await flushPromiseJobs();
  assert.deepEqual(errors, ["active subscribe failed"]);
  disposeActive();

  const disposed = controlledSubscription();
  const disposeBeforeReject = startModelStreamSubscription({
    subscribe: disposed.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => false,
    onError: (message) => errors.push(message)
  });
  disposeBeforeReject();
  disposed.registration.reject(new Error("disposed subscribe failed"));
  await flushPromiseJobs();

  assert.deepEqual(errors, ["active subscribe failed"]);
  assert.equal(snapshots.length, 0);
  accumulator.dispose();
});

test("a durable terminal refresh clears the stream only after synchronization", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  const synchronized = deferred<boolean>();
  const done: string[] = [];

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: (sessionId) => {
      done.push(sessionId);
      return synchronized.promise;
    },
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "synthesis", { delta: "final answer" }));
  listener(event("session-A", "outer-run", { delta: "", done: true }));

  assert.equal(snapshotText(accumulator.snapshot()), "final answer");
  assert.deepEqual(done, ["session-A"]);
  synchronized.resolve(true);
  await flushPromiseJobs();

  assert.equal(snapshotText(accumulator.snapshot()), "");
  assert.equal(snapshotText(snapshots.at(-1)!), "");
  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("an unconfirmed terminal refresh preserves the streamed answer", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => false,
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "request-A", { delta: "partial answer" }));
  listener(event("session-A", "request-A", { delta: "", done: true }));
  await flushPromiseJobs();

  assert.equal(snapshotText(accumulator.snapshot()), "partial answer");
  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("a delayed terminal synchronization cannot clear a buffered continuation", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  const synchronized = deferred<boolean>();

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => synchronized.promise,
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "synthesis", { delta: "old answer" }));
  listener(event("session-A", "outer-run", { delta: "", done: true }));
  listener(event("session-A", "synthesis", { delta: " continues" }));
  assert.equal(accumulator.bufferedFragmentCount, 1);
  synchronized.resolve(true);
  await flushPromiseJobs();

  assert.equal(accumulator.bufferedFragmentCount, 1);
  accumulator.finish();
  assert.equal(snapshotText(accumulator.snapshot()), "old answer continues");
  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("a request arriving before terminal synchronization replaces the old stream", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  const synchronized = deferred<boolean>();

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => synchronized.promise,
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "request-old", { delta: "old answer" }));
  listener(event("session-A", "request-old", { delta: "", done: true }));
  listener(event("session-A", "request-new", { delta: "new answer" }));
  assert.equal(snapshotText(accumulator.snapshot()), "new answer");

  synchronized.resolve(true);
  await flushPromiseJobs();
  assert.equal(snapshotText(accumulator.snapshot()), "new answer");
  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("a hidden durable terminal cannot republish the completed stream on reveal", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  accumulator.setVisible(false);

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => true,
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "request-A", { delta: "hidden answer" }));
  listener(event("session-A", "request-A", { delta: "", done: true }));
  await flushPromiseJobs();

  assert.equal(snapshotText(accumulator.snapshot()), "");
  accumulator.setVisible(true);
  assert.equal(snapshotText(snapshots.at(-1)!), "");
  assert.equal(snapshots.some((snapshot) => snapshotText(snapshot) === "hidden answer"), false);
  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});

test("a hidden terminal revealed before delayed durable sync is cleared afterward", async () => {
  const snapshots: StreamingMarkdownSnapshot[] = [];
  const accumulator = accumulatorFixture(snapshots);
  const router = new ModelStreamEventRouter();
  const controlled = controlledSubscription();
  const synchronized = deferred<boolean>();
  accumulator.setVisible(false);

  const dispose = startModelStreamSubscription({
    subscribe: controlled.subscribe,
    router,
    accumulator,
    activeSessionId: () => "session-A",
    onStreamDone: () => synchronized.promise,
    onError: () => assert.fail("terminal synchronization should not fail")
  });
  const listener = controlled.listener();
  listener(event("session-A", "request-A", { delta: "hidden answer" }));
  listener(event("session-A", "request-A", { delta: "", done: true }));

  assert.equal(snapshots.length, 0);
  assert.equal(snapshotText(accumulator.snapshot()), "hidden answer");
  accumulator.setVisible(true);
  assert.equal(snapshotText(snapshots.at(-1)!), "hidden answer");

  synchronized.resolve(true);
  await flushPromiseJobs();
  assert.equal(snapshotText(accumulator.snapshot()), "");
  assert.equal(snapshotText(snapshots.at(-1)!), "");

  controlled.registration.resolve(() => {});
  await flushPromiseJobs();
  dispose();
  accumulator.dispose();
});
