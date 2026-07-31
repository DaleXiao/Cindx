import assert from "node:assert/strict";
import test from "node:test";
import { ArtifactImagePreviewCache } from "../src/utils/artifactImagePreviewCache.ts";

test("artifact image cache coalesces reads and uses the backend-authoritative MIME", async () => {
  let reads = 0;
  let release: ((bytes: Uint8Array) => void) | undefined;
  const loaded = new Promise<{ bytes: Uint8Array; mimeType: string }>((resolve) => {
    release = (bytes) => resolve({ bytes, mimeType: "image/png" });
  });
  const created: string[] = [];
  const cache = new ArtifactImagePreviewCache({
    load: async () => {
      reads += 1;
      return loaded;
    },
    createUrl: (bytes, mimeType) => {
      const url = `blob:${mimeType}:${bytes.byteLength}`;
      created.push(url);
      return url;
    },
    revokeUrl: () => {},
    entryLimit: 4,
    byteLimit: 1024
  });

  const first = cache.acquire("/one.jpg");
  const concurrent = cache.acquire("/one.jpg");
  release?.(new Uint8Array(64));

  const firstHandle = await first;
  const concurrentHandle = await concurrent;
  const cachedHandle = await cache.acquire("/one.jpg");
  assert.equal(firstHandle.url, "blob:image/png:64");
  assert.equal(concurrentHandle.url, "blob:image/png:64");
  assert.equal(cachedHandle.url, "blob:image/png:64");
  assert.equal(reads, 1);
  assert.deepEqual(created, ["blob:image/png:64"]);
  firstHandle.release();
  concurrentHandle.release();
  cachedHandle.release();
});

test("artifact image cache bounds bytes and entries with LRU revocation", async () => {
  const revoked: string[] = [];
  let created = 0;
  const cache = new ArtifactImagePreviewCache({
    load: async (path) => ({
      bytes: new Uint8Array(path.endsWith("large.png") ? 70 : 40),
      mimeType: "image/png"
    }),
    createUrl: (_bytes, mimeType) => `blob:${mimeType}:${++created}`,
    revokeUrl: (url) => revoked.push(url),
    entryLimit: 2,
    byteLimit: 100
  });

  const first = await cache.acquire("/first.png");
  const second = await cache.acquire("/second.png");
  const promoted = await cache.acquire("/first.png");
  const large = await cache.acquire("/large.png");

  assert.deepEqual(revoked, []);
  second.release();
  assert.deepEqual(revoked, [second.url]);
  first.release();
  assert.deepEqual(revoked, [second.url]);
  promoted.release();
  assert.deepEqual(revoked, [second.url, first.url]);
  large.release();
});

test("artifact image cache does not retain failed loads", async () => {
  let reads = 0;
  const cache = new ArtifactImagePreviewCache({
    load: async () => {
      reads += 1;
      if (reads === 1) throw new Error("temporary read failure");
      return { bytes: new Uint8Array(8), mimeType: "image/png" };
    },
    createUrl: () => "blob:recovered",
    revokeUrl: () => {},
    entryLimit: 1,
    byteLimit: 16
  });

  await assert.rejects(cache.acquire("/retry.png"), /temporary read failure/);
  const recovered = await cache.acquire("/retry.png");
  assert.equal(recovered.url, "blob:recovered");
  assert.equal(reads, 2);
  recovered.release();
});
