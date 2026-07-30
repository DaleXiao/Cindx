import {
  StreamingMarkdownTailBuffer,
  type StreamingMarkdownTailBranch,
  type StreamingMarkdownTailFragment
} from "./streamingMarkdownTailBuffer.ts";
export {
  STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE,
  STREAMING_MARKDOWN_LIVE_PARSE_BUDGET,
  STREAMING_MARKDOWN_ROPE_DEPTH,
  STREAMING_MARKDOWN_ROPE_FANOUT
} from "./streamingMarkdownTailBuffer.ts";
export type {
  StreamingMarkdownTailBranch,
  StreamingMarkdownTailFragment,
  StreamingMarkdownTailLeaf,
  StreamingMarkdownTailNode
} from "./streamingMarkdownTailBuffer.ts";
export const STREAMING_MARKDOWN_CHUNK_TARGET = 1_600;
export const STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT = 512;
export const STREAMING_MARKDOWN_PREVIEW_LIMIT = 2_048;
export type StreamingMarkdownChunk = Readonly<{ id: number; content: string }>;
export type StreamingMarkdownSnapshot = Readonly<{
  settledChunks: readonly StreamingMarkdownChunk[];
  parsedTail: StreamingMarkdownTailFragment;
  deferredTailRoot: StreamingMarkdownTailBranch;
  deferredTailOpen: StreamingMarkdownTailFragment;
  deferredInFence: boolean;
  tailId: number;
  preview: string;
  totalLength: number;
  mutableLength: number;
  parseWork: number;
  joinWork: number;
}>;
const EMPTY_SETTLED = Object.freeze([]) as readonly StreamingMarkdownChunk[];

export class StreamingMarkdownModel {
  private settledChunks: readonly StreamingMarkdownChunk[] = EMPTY_SETTLED;
  private readonly tail = new StreamingMarkdownTailBuffer();
  private tailId = 0;
  private preview = "";
  private totalLength = 0;
  private fenceCharacter = "";
  private fenceLength = 0;
  private lineHasContent = false;
  private lineMarkerCharacter = "";
  private lineMarkerLength = 0;
  private lineMarkerComplete = false;
  private pendingBoundary = false;
  private scannedCharacters = 0;
  private currentSnapshot = this.captureSnapshot();

  get scanWork() { return this.scannedCharacters; }
  get parseWork() { return this.tail.parseWork; }
  get joinWork() { return this.tail.joinWork; }
  snapshot() { return this.currentSnapshot; }

  append(delta: string) {
    if (!delta) return this.currentSnapshot;
    if (this.pendingBoundary) this.settleTail();
    this.scannedCharacters += delta.length;
    this.totalLength += delta.length;
    if (this.preview.length < STREAMING_MARKDOWN_PREVIEW_LIMIT) {
      this.preview += delta.slice(0, STREAMING_MARKDOWN_PREVIEW_LIMIT - this.preview.length);
    }
    let segmentStart = 0;
    for (let index = 0; index < delta.length; index += 1) {
      const character = delta[index];
      if (character !== "\n") {
        this.processLineCharacter(character);
        continue;
      }
      this.tail.append(delta.slice(segmentStart, index + 1));
      this.processCompletedLine();
      segmentStart = index + 1;
      if (this.pendingBoundary && segmentStart < delta.length) this.settleTail();
    }
    if (segmentStart < delta.length) this.tail.append(delta.slice(segmentStart));
    this.tail.refresh(this.pendingBoundary);
    this.currentSnapshot = this.captureSnapshot();
    return this.currentSnapshot;
  }

  reset() {
    this.settledChunks = EMPTY_SETTLED;
    this.tail.reset();
    this.tailId = 0;
    this.preview = "";
    this.totalLength = 0;
    this.fenceCharacter = "";
    this.fenceLength = 0;
    this.resetLineState();
    this.pendingBoundary = false;
    this.scannedCharacters = 0;
    this.currentSnapshot = this.captureSnapshot();
    return this.currentSnapshot;
  }

  private processLineCharacter(character: string) {
    if (!this.lineHasContent) {
      if (character.trim() === "") return;
      this.lineHasContent = true;
      if (character === "`" || character === "~") {
        this.lineMarkerCharacter = character;
        this.lineMarkerLength = 1;
      } else {
        this.lineMarkerComplete = true;
      }
      return;
    }
    if (this.lineMarkerComplete) return;
    if (character === this.lineMarkerCharacter) this.lineMarkerLength += 1;
    else this.lineMarkerComplete = true;
  }

  private processCompletedLine() {
    if (this.lineMarkerLength >= 3) {
      if (!this.fenceCharacter) {
        this.fenceCharacter = this.lineMarkerCharacter;
        this.fenceLength = this.lineMarkerLength;
      } else if (
        this.lineMarkerCharacter === this.fenceCharacter &&
        this.lineMarkerLength >= this.fenceLength
      ) {
        this.fenceCharacter = "";
        this.fenceLength = 0;
      }
    }
    this.pendingBoundary =
      this.settledChunks.length < STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT &&
      !this.fenceCharacter &&
      !this.lineHasContent &&
      this.tail.length >= STREAMING_MARKDOWN_CHUNK_TARGET;
    this.resetLineState();
  }

  private settleTail() {
    const chunk = Object.freeze({ id: this.tailId, content: this.tail.settle() });
    this.settledChunks = Object.freeze([...this.settledChunks, chunk]);
    this.tailId += 1;
    this.fenceCharacter = "";
    this.fenceLength = 0;
    this.resetLineState();
    this.pendingBoundary = false;
  }

  private resetLineState() {
    this.lineHasContent = false;
    this.lineMarkerCharacter = "";
    this.lineMarkerLength = 0;
    this.lineMarkerComplete = false;
  }

  private captureSnapshot(): StreamingMarkdownSnapshot {
    return Object.freeze({
      settledChunks: this.settledChunks,
      parsedTail: this.tail.parsedTail,
      deferredTailRoot: this.tail.deferredTailRoot,
      deferredTailOpen: this.tail.deferredTailOpen,
      deferredInFence: this.fenceCharacter.length > 0,
      tailId: this.tailId,
      preview: this.preview,
      totalLength: this.totalLength,
      mutableLength: this.tail.length,
      parseWork: this.tail.parseWork,
      joinWork: this.tail.joinWork
    });
  }
}
