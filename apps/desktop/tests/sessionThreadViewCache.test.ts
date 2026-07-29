import assert from "node:assert/strict";
import test from "node:test";
import type { AgentOutputArtifactView } from "../src/tauriTypes.ts";
import { SessionThreadViewCache } from "../src/components/sessionThreadViewCache.ts";

function artifact(id: string): AgentOutputArtifactView {
  return {
    id,
    path: `/tmp/${id}.txt`,
    sourcePath: null,
    toolName: "write_file",
    status: "ready",
    timestampMs: 1,
    runId: "run-1",
    version: 1,
    kind: "file"
  };
}

test("session thread row heights remain bounded by row and session", () => {
  const cache = new SessionThreadViewCache(2, 2);
  cache.rememberRowHeight("a", "a-1", 40);
  cache.rememberRowHeight("a", "a-2", 50);
  assert.deepEqual(cache.rememberRowHeight("a", "a-3", 60), [
    { sessionId: "a", rowKey: "a-1" }
  ]);
  assert.equal(cache.rowHeight("a", "a-1"), undefined);
  assert.equal(cache.rowHeight("a", "a-3"), 60);

  cache.rememberRowHeight("b", "b-1", 70);
  const evicted = cache.rememberRowHeight("c", "c-1", 80);
  assert.deepEqual(evicted, [
    { sessionId: "a", rowKey: "a-2" },
    { sessionId: "a", rowKey: "a-3" }
  ]);
  assert.equal(cache.rowHeight("a", "a-3"), undefined);
  assert.equal(cache.updateViewportWidth(700), false);
  assert.equal(cache.updateViewportWidth(700.4), false);
  assert.equal(cache.rowHeight("c", "c-1"), 80);
  assert.equal(cache.updateViewportWidth(699), true);
  assert.equal(cache.rowHeight("c", "c-1"), undefined);
});

test("session artifacts reuse stable arrays and evict old sessions", () => {
  const cache = new SessionThreadViewCache(2, 2);
  const first = [artifact("first")];
  assert.equal(cache.rememberArtifacts("a", first), first);
  assert.equal(cache.rememberArtifacts("a", [artifact("first")]), first);
  assert.equal(cache.artifactsFor("a"), first);

  cache.rememberArtifacts("b", [artifact("second")]);
  cache.rememberArtifacts("c", [artifact("third")]);
  assert.equal(cache.artifactsFor("a"), null);
});

test("session artifact loads coalesce rapid refreshes without a fixed delay", async () => {
  const cache = new SessionThreadViewCache(2, 2);
  const resolvers: Array<(artifacts: AgentOutputArtifactView[]) => void> = [];
  let loadCount = 0;
  const load = () =>
    new Promise<AgentOutputArtifactView[]>((resolve) => {
      loadCount += 1;
      resolvers.push(resolve);
    });

  const first = cache.loadArtifacts("a", load);
  const second = cache.loadArtifacts("a", load);
  assert.equal(first, second);
  assert.equal(loadCount, 1);

  resolvers.shift()?.([artifact("old")]);
  await Promise.resolve();
  assert.equal(loadCount, 2);
  resolvers.shift()?.([artifact("latest")]);
  assert.deepEqual(await first, [artifact("latest")]);
  assert.equal(cache.artifactsFor("a")?.[0]?.id, "latest");
});
