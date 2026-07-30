import assert from "node:assert/strict";
import test from "node:test";
import {
  STREAMING_MARKDOWN_CHUNK_TARGET,
  STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE,
  STREAMING_MARKDOWN_LIVE_PARSE_BUDGET,
  STREAMING_MARKDOWN_PREVIEW_LIMIT,
  STREAMING_MARKDOWN_ROPE_DEPTH,
  STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT,
  StreamingMarkdownModel,
  type StreamingMarkdownSnapshot,
  type StreamingMarkdownTailLeaf,
  type StreamingMarkdownTailNode
} from "../src/components/streamingMarkdownModel.ts";

function deferredLeaves(root: StreamingMarkdownTailNode) {
  const leaves: StreamingMarkdownTailLeaf[] = [];
  const stack = [root];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node) continue;
    if (node.kind === "leaf") leaves.push(node);
    else {
      for (let index = node.children.length - 1; index >= 0; index -= 1) {
        stack.push(node.children[index]);
      }
    }
  }
  return leaves;
}

function deferredText(root: StreamingMarkdownTailNode) {
  return deferredLeaves(root).map((leaf) => leaf.content).join("");
}

function mutableText(snapshot: StreamingMarkdownSnapshot) {
  return [
    snapshot.parsedTail.content,
    deferredText(snapshot.deferredTailRoot),
    snapshot.deferredTailOpen.content
  ].join("");
}

function snapshotText(snapshot: StreamingMarkdownSnapshot) {
  return `${snapshot.settledChunks.map((chunk) => chunk.content).join("")}${mutableText(snapshot)}`;
}

function legacyStreamingChunks(content: string) {
  if (content.length <= STREAMING_MARKDOWN_CHUNK_TARGET) return [content];
  const chunks: string[] = [];
  let start = 0;
  let offset = 0;
  let fenceCharacter = "";
  let fenceLength = 0;
  for (const line of content.match(/.*(?:\n|$)/g) ?? []) {
    if (!line) continue;
    const trimmed = line.replace(/\n$/, "").trim();
    const fence = trimmed.match(/^(`{3,}|~{3,})/);
    if (fence) {
      const marker = fence[1];
      if (!fenceCharacter) {
        fenceCharacter = marker[0];
        fenceLength = marker.length;
      } else if (marker[0] === fenceCharacter && marker.length >= fenceLength) {
        fenceCharacter = "";
        fenceLength = 0;
      }
    }
    offset += line.length;
    if (
      !fenceCharacter &&
      trimmed === "" &&
      offset - start >= STREAMING_MARKDOWN_CHUNK_TARGET
    ) {
      chunks.push(content.slice(start, offset));
      start = offset;
    }
  }
  if (start < content.length) chunks.push(content.slice(start));
  return chunks.length > 0 ? chunks : [content];
}

function assertMatchesLegacy(model: StreamingMarkdownModel, source: string) {
  const snapshot = model.snapshot();
  const legacy = legacyStreamingChunks(source);
  assert.deepEqual(
    snapshot.settledChunks.map((chunk) => chunk.content),
    legacy.slice(0, -1)
  );
  assert.equal(mutableText(snapshot), legacy.at(-1));
  assert.equal(snapshotText(snapshot), source);
  assert.equal(snapshot.totalLength, source.length);
  assert.equal(snapshot.mutableLength, mutableText(snapshot).length);
  assert.equal(snapshot.parseWork, model.parseWork);
  assert.equal(snapshot.joinWork, model.joinWork);
}

function feedAndCompare(source: string, sizes: readonly number[]) {
  const model = new StreamingMarkdownModel();
  let offset = 0;
  let visible = "";
  let sizeIndex = 0;
  while (offset < source.length) {
    const size = Math.max(1, sizes[sizeIndex % sizes.length] ?? 1);
    const delta = source.slice(offset, offset + size);
    offset += delta.length;
    sizeIndex += 1;
    visible += delta;
    model.append(delta);
    assertMatchesLegacy(model, visible);
  }
  return model;
}

function deterministicSizes(length: number) {
  let state = 0x9e3779b9;
  const sizes: number[] = [];
  let covered = 0;
  while (covered < length) {
    state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;
    const size = 1 + (state % 97);
    sizes.push(size);
    covered += size;
  }
  return sizes;
}

const richMarkdown = [
  "# Incremental Markdown\r\n",
  "你好，世界 👋🏽 — café — مرحبا\r\n",
  `${"paragraph content ".repeat(115)}\r\n\r\n`,
  "- first item\r\n- second item\r\n\r\n",
  "| Name | Value |\r\n| --- | ---: |\r\n| alpha | 1 |\r\n\r\n",
  "> quoted line\r\n> continuation\r\n\r\n",
  "```mermaid\r\ngraph TD\r\n  A --> B\r\n\r\n  B --> C\r\n```\r\n\r\n",
  `${"between diagrams ".repeat(110)}\n\n`,
  "~~~mindmap\n# Root\n## Child\n\n### Grandchild\n~~~\n\n",
  "Inline math $x^2 + y^2$ and display math:\n\n$$\\int_0^1 x^2 dx$$\n\n",
  `${"final paragraph ".repeat(130)}\n`
].join("");

test("matches the legacy splitter for single-byte, Unicode, and structured deltas", () => {
  const source = richMarkdown.replace(/\r\n/g, "\n");
  feedAndCompare(source, [1]);
  feedAndCompare(source, [2, 3, 5, 8, 13, 21]);
  feedAndCompare(source, deterministicSizes(source.length));
});

test("handles CRLF boundaries without losing content", () => {
  const source = `${"crlf paragraph ".repeat(120)}\r\n\r\n- next\r\n`;
  const model = new StreamingMarkdownModel();
  let visible = "";
  for (const size of deterministicSizes(source.length)) {
    if (visible.length >= source.length) break;
    const delta = source.slice(visible.length, visible.length + size);
    visible += delta;
    const snapshot = model.append(delta);
    assert.equal(snapshotText(snapshot), visible);
  }
  assert.equal(model.snapshot().settledChunks.length, 1);
  assert.ok(model.snapshot().settledChunks[0].content.endsWith("\r\n\r\n"));
});

test("never settles a blank boundary inside backtick or tilde fences", () => {
  for (const fence of ["```", "~~~~"]) {
    const source = [
      `${fence}mermaid\n`,
      `${"graph content\n".repeat(150)}`,
      "\n",
      `${"still fenced\n".repeat(20)}`,
      `${fence}\n`,
      "\n",
      "tail"
    ].join("");
    const model = feedAndCompare(source, [7, 1, 29, 3]);
    assert.equal(model.snapshot().settledChunks.length, 1);
    assert.ok(model.snapshot().settledChunks[0].content.endsWith(`${fence}\n\n`));
  }
});

test("keeps an end boundary mutable until the next non-empty delta", () => {
  const model = new StreamingMarkdownModel();
  const boundary = `${"a".repeat(STREAMING_MARKDOWN_CHUNK_TARGET)}\n\n`;
  const pending = model.append(boundary);

  assert.equal(pending.settledChunks.length, 0);
  assert.equal(mutableText(pending), boundary);
  assert.equal(pending.tailId, 0);
  assert.strictEqual(model.append(""), pending);

  const advanced = model.append("next");
  assert.equal(advanced.settledChunks.length, 1);
  assert.equal(advanced.settledChunks[0].content, boundary);
  assert.equal(advanced.settledChunks[0].id, 0);
  assert.equal(mutableText(advanced), "next");
  assert.equal(advanced.tailId, 1);
});

test("preserves settled chunk objects and arrays until another boundary settles", () => {
  const model = new StreamingMarkdownModel();
  const firstBoundary = `${"a".repeat(STREAMING_MARKDOWN_CHUNK_TARGET)}\n\n`;
  model.append(firstBoundary);
  const firstSettled = model.append("tail");
  const firstChunk = firstSettled.settledChunks[0];
  const firstArray = firstSettled.settledChunks;

  const extended = model.append(" grows");
  assert.strictEqual(extended.settledChunks, firstArray);
  assert.strictEqual(extended.settledChunks[0], firstChunk);

  model.append(`${"b".repeat(STREAMING_MARKDOWN_CHUNK_TARGET)}\n\n`);
  const secondSettled = model.append("final");
  assert.notStrictEqual(secondSettled.settledChunks, firstArray);
  assert.strictEqual(secondSettled.settledChunks[0], firstChunk);
  assert.equal(secondSettled.settledChunks[1].id, 1);
  assert.equal(secondSettled.tailId, 2);
});

test("keeps ordinary mutable tails fully parsed without a duplicate full-tail field", () => {
  const model = new StreamingMarkdownModel();
  const parts = ["first token", " — 你好", "\n\n", "final"];
  let expected = "";

  for (const part of parts) {
    expected += part;
    const snapshot = model.append(part);
    assert.equal(snapshot.parsedTail.content, expected);
    assert.equal(snapshot.deferredTailRoot.length, 0);
    assert.equal(snapshot.deferredTailOpen.content, "");
    assert.equal(snapshot.deferredInFence, false);
    assert.equal(mutableText(snapshot), expected);
    assert.equal("tail" in snapshot, false);
  }
});

test("keeps parsed checkpoints and persistent rope identities stable between thresholds", () => {
  const model = new StreamingMarkdownModel();
  const checkpoint = model.append("x".repeat(STREAMING_MARKDOWN_LIVE_PARSE_BUDGET * 2));
  const parsedTail = checkpoint.parsedTail;

  const firstDeferred = model.append("a".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE));
  const firstRoot = firstDeferred.deferredTailRoot;
  const firstLeaf = deferredLeaves(firstRoot)[0];
  assert.strictEqual(firstDeferred.parsedTail, parsedTail);
  assert.equal(firstRoot.depth, STREAMING_MARKDOWN_ROPE_DEPTH);
  assert.equal(firstRoot.length, STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE);
  assert.equal(firstLeaf.content, "a".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE));
  assert.equal(firstDeferred.deferredTailOpen.content, "");

  const withOpen = model.append("bc");
  assert.strictEqual(withOpen.parsedTail, parsedTail);
  assert.strictEqual(withOpen.deferredTailRoot, firstRoot);
  assert.strictEqual(deferredLeaves(withOpen.deferredTailRoot)[0], firstLeaf);
  const firstOpen = withOpen.deferredTailOpen;

  const extendedOpen = model.append("de");
  assert.strictEqual(extendedOpen.parsedTail, parsedTail);
  assert.strictEqual(extendedOpen.deferredTailRoot, firstRoot);
  assert.notStrictEqual(extendedOpen.deferredTailOpen, firstOpen);
  assert.equal(extendedOpen.deferredTailOpen.id, firstOpen.id);
  assert.equal(extendedOpen.deferredTailOpen.content, "bcde");

  const secondDeferred = model.append(
    "f".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE - 4)
  );
  assert.strictEqual(secondDeferred.parsedTail, parsedTail);
  assert.notStrictEqual(secondDeferred.deferredTailRoot, firstRoot);
  assert.equal(secondDeferred.deferredTailRoot.id, firstRoot.id);
  const secondLeaves = deferredLeaves(secondDeferred.deferredTailRoot);
  assert.equal(secondLeaves.length, 2);
  assert.strictEqual(secondLeaves[0], firstLeaf);
  assert.equal(
    secondLeaves[1].content,
    `bcde${"f".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE - 4)}`
  );
  assert.equal(deferredText(secondDeferred.deferredTailRoot),
    `${firstLeaf.content}${secondLeaves[1].content}`);
  assert.equal(secondDeferred.deferredTailOpen.content, "");
});

test("never splits emoji surrogate pairs or CRLF across rendered tail fragments", () => {
  const assertSafeFragmentBoundaries = (snapshot: StreamingMarkdownSnapshot) => {
    const fragments = [
      ...deferredLeaves(snapshot.deferredTailRoot).map((leaf) => leaf.content),
      snapshot.deferredTailOpen.content
    ].filter(Boolean);
    for (let index = 0; index < fragments.length - 1; index += 1) {
      const left = fragments[index];
      const right = fragments[index + 1];
      const leftCodeUnit = left.charCodeAt(left.length - 1);
      const rightCodeUnit = right.charCodeAt(0);
      assert.equal(
        leftCodeUnit >= 0xd800 && leftCodeUnit <= 0xdbff &&
          rightCodeUnit >= 0xdc00 && rightCodeUnit <= 0xdfff,
        false
      );
      assert.equal(left.endsWith("\r") && right.startsWith("\n"), false);
    }
  };

  for (const splitAcrossDeltas of [false, true]) {
    const model = new StreamingMarkdownModel();
    model.append("x".repeat(STREAMING_MARKDOWN_LIVE_PARSE_BUDGET * 2));
    const prefix = "a".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE - 1);
    if (splitAcrossDeltas) {
      model.append(`${prefix}\ud83d`);
      model.append("\ude00");
    } else {
      model.append(`${prefix}😀`);
    }
    const snapshot = model.snapshot();
    assert.equal(snapshotText(snapshot).endsWith(`${prefix}😀`), true);
    assertSafeFragmentBoundaries(snapshot);
    assert.equal(snapshot.deferredTailOpen.content, "😀");
  }

  const crlfModel = new StreamingMarkdownModel();
  crlfModel.append("x".repeat(STREAMING_MARKDOWN_LIVE_PARSE_BUDGET * 2));
  const crlfPrefix = "b".repeat(STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE - 1);
  crlfModel.append(`${crlfPrefix}\r`);
  crlfModel.append("\nnext");
  const crlfSnapshot = crlfModel.snapshot();
  assert.equal(snapshotText(crlfSnapshot).endsWith(`${crlfPrefix}\r\nnext`), true);
  assertSafeFragmentBoundaries(crlfSnapshot);
  assert.equal(crlfSnapshot.deferredTailOpen.content, "\r\nnext");
});

test("uses geometric checkpoints with linear work for a one MiB unbroken line", () => {
  const sourceLength = 1 * 1_024 * 1_024;
  const source = "x".repeat(sourceLength);
  const model = new StreamingMarkdownModel();
  const checkpointLengths: number[] = [];
  let previousParsedLength = 0;

  for (let offset = 0; offset < source.length; offset += 1_024) {
    const snapshot = model.append(source.slice(offset, offset + 1_024));
    if (
      snapshot.parsedTail.content.length !== previousParsedLength &&
      snapshot.parsedTail.content.length >= STREAMING_MARKDOWN_LIVE_PARSE_BUDGET
    ) {
      checkpointLengths.push(snapshot.parsedTail.content.length);
    }
    previousParsedLength = snapshot.parsedTail.content.length;
  }

  const expectedCheckpoints: number[] = [];
  for (
    let length = STREAMING_MARKDOWN_LIVE_PARSE_BUDGET;
    length <= sourceLength;
    length *= 2
  ) {
    expectedCheckpoints.push(length);
  }
  const startupBound =
    (STREAMING_MARKDOWN_LIVE_PARSE_BUDGET *
      (STREAMING_MARKDOWN_LIVE_PARSE_BUDGET + 1)) /
    2;
  const snapshot = model.snapshot();

  assert.deepEqual(checkpointLengths, expectedCheckpoints);
  assert.equal(snapshotText(snapshot), source);
  assert.equal(snapshot.settledChunks.length, 0);
  assert.equal(snapshot.deferredInFence, false);
  assert.ok(model.parseWork <= startupBound + sourceLength * 3);
  assert.ok(model.joinWork <= startupBound + sourceLength * 4);
});

test("keeps rope string work linear across an eight MiB unbroken line", () => {
  const sourceLength = 8 * 1_024 * 1_024 - STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE;
  const source = "r".repeat(sourceLength);
  const model = new StreamingMarkdownModel();
  for (let offset = 0; offset < source.length; offset += 4_096) {
    model.append(source.slice(offset, offset + 4_096));
  }

  const snapshot = model.snapshot();
  assert.equal(snapshotText(snapshot), source);
  assert.equal(
    snapshot.parsedTail.content.length +
      snapshot.deferredTailRoot.length +
      snapshot.deferredTailOpen.content.length,
    snapshot.mutableLength
  );
  assert.ok(model.parseWork <= sourceLength * 3);
  assert.ok(model.joinWork <= sourceLength * 4);
  console.log(
    JSON.stringify({
      schema: "cindx.frontend-streaming-markdown-scaling.v1",
      source_length: sourceLength,
      parse_work: model.parseWork,
      join_work: model.joinWork
    })
  );
});

test("bounds settled metadata before continuing with geometric tail checkpoints", () => {
  const model = new StreamingMarkdownModel();
  const block = `${"p".repeat(STREAMING_MARKDOWN_CHUNK_TARGET)}\n\n`;
  const blockCount = STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT + 24;
  for (let index = 0; index < blockCount; index += 1) {
    model.append(block);
  }

  const snapshot = model.snapshot();
  assert.equal(snapshot.settledChunks.length, STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT);
  assert.equal(snapshotText(snapshot), block.repeat(blockCount));
  assert.ok(snapshot.mutableLength > STREAMING_MARKDOWN_CHUNK_TARGET);
});

test("bounds an unclosed fence and returns to normal parsing after a safe boundary", () => {
  const sourceLength = 1 * 1_024 * 1_024;
  const prefix = "```mermaid\r\n";
  const bodyUnit = "graph TD\r\n  A --> B\r\n\r\n";
  const source = `${prefix}${bodyUnit.repeat(
    Math.ceil((sourceLength - prefix.length) / bodyUnit.length)
  )}`.slice(0, sourceLength);
  const model = new StreamingMarkdownModel();
  const sizes = [1, 17, 4_096];
  let sizeIndex = 0;

  for (let offset = 0; offset < source.length; ) {
    const size = sizes[sizeIndex % sizes.length];
    const delta = source.slice(offset, offset + size);
    offset += delta.length;
    sizeIndex += 1;
    model.append(delta);
  }

  const startupBound =
    (STREAMING_MARKDOWN_LIVE_PARSE_BUDGET *
      (STREAMING_MARKDOWN_LIVE_PARSE_BUDGET + 1)) /
    2;
  const streamed = model.snapshot();
  assert.equal(streamed.settledChunks.length, 0);
  assert.equal(streamed.deferredInFence, true);
  assert.equal(snapshotText(streamed), source);
  assert.ok(model.parseWork <= startupBound + source.length * 3);
  assert.ok(model.joinWork <= startupBound + source.length * 4);

  const boundary = "\r\n```\r\n\r\n";
  const pending = model.append(boundary);
  assert.equal(pending.settledChunks.length, 0);
  assert.equal(pending.deferredInFence, false);
  assert.equal(snapshotText(pending), `${source}${boundary}`);

  const advanced = model.append("after");
  assert.equal(advanced.settledChunks.length, 1);
  assert.equal(advanced.settledChunks[0].content, `${source}${boundary}`);
  assert.equal(advanced.parsedTail.content, "after");
  assert.equal(advanced.deferredTailRoot.length, 0);
  assert.equal(advanced.deferredTailOpen.content, "");
  assert.equal(advanced.deferredInFence, false);
  assert.equal(snapshotText(advanced), `${source}${boundary}after`);
});

test("bounds the minimap preview while retaining exact content lengths", () => {
  const source = `prefix-${"界".repeat(STREAMING_MARKDOWN_PREVIEW_LIMIT + 500)}`;
  const model = feedAndCompare(source, [17, 31]);
  const snapshot = model.snapshot();

  assert.equal(snapshot.preview, source.slice(0, STREAMING_MARKDOWN_PREVIEW_LIMIT));
  assert.equal(snapshot.preview.length, STREAMING_MARKDOWN_PREVIEW_LIMIT);
  assert.equal(snapshot.totalLength, source.length);
});

test("reset clears content, identities, preview, and scan accounting", () => {
  const model = new StreamingMarkdownModel();
  model.append(`${"x".repeat(STREAMING_MARKDOWN_CHUNK_TARGET)}\n\n`);
  model.append("tail");
  assert.ok(model.scanWork > 0);

  const reset = model.reset();
  assert.deepEqual(reset.settledChunks, []);
  assert.equal(mutableText(reset), "");
  assert.equal(reset.tailId, 0);
  assert.equal(reset.preview, "");
  assert.equal(reset.totalLength, 0);
  assert.equal(reset.mutableLength, 0);
  assert.equal(model.scanWork, 0);
  assert.equal(model.parseWork, 0);
  assert.equal(model.joinWork, 0);

  const next = model.append("new");
  assert.equal(next.tailId, 0);
  assert.equal(mutableText(next), "new");
});

test("scan work is deterministic and linear in appended input", () => {
  const source = Array.from({ length: 8_000 }, (_, index) =>
    index % 11 === 0 ? `line ${index}\n` : `value-${index} `
  ).join("");
  const model = feedAndCompare(source, deterministicSizes(source.length));

  assert.ok(model.scanWork >= source.length);
  assert.ok(
    model.scanWork <= source.length * 2,
    `expected at most two scans per code unit, got ${model.scanWork}/${source.length}`
  );
});
