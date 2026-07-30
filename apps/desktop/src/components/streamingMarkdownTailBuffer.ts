export const STREAMING_MARKDOWN_LIVE_PARSE_BUDGET = 8 * 1_024;
export const STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE = 4 * 1_024;
export const STREAMING_MARKDOWN_ROPE_FANOUT = 32;
export const STREAMING_MARKDOWN_ROPE_DEPTH = 6;

export type StreamingMarkdownTailFragment = Readonly<{
  id: number;
  content: string;
}>;

export type StreamingMarkdownTailLeaf = Readonly<{
  kind: "leaf";
  id: number;
  content: string;
  length: number;
}>;

export type StreamingMarkdownTailBranch = Readonly<{
  kind: "branch";
  id: number;
  children: readonly StreamingMarkdownTailNode[];
  depth: number;
  length: number;
}>;

export type StreamingMarkdownTailNode =
  | StreamingMarkdownTailLeaf
  | StreamingMarkdownTailBranch;

const EMPTY_NODES = Object.freeze([]) as readonly StreamingMarkdownTailNode[];
const INITIAL_PARSED = Object.freeze({ id: 0, content: "" });
const INITIAL_OPEN = Object.freeze({ id: 1, content: "" });
const INITIAL_ROOT = Object.freeze({
  kind: "branch" as const,
  id: 2,
  children: EMPTY_NODES,
  depth: STREAMING_MARKDOWN_ROPE_DEPTH,
  length: 0
});
function needsBoundaryCarry(content: string) {
  const codeUnit = content.charCodeAt(content.length - 1);
  return content.endsWith("\r") || codeUnit >= 0xd800 && codeUnit <= 0xdbff;
}

export class StreamingMarkdownTailBuffer {
  private parsed: StreamingMarkdownTailFragment = INITIAL_PARSED;
  private root: StreamingMarkdownTailBranch = INITIAL_ROOT;
  private open: StreamingMarkdownTailFragment = INITIAL_OPEN;
  private nextFragmentId = 3;
  private leafCount = 0;
  private tailLength = 0;
  private nextParseAt = STREAMING_MARKDOWN_LIVE_PARSE_BUDGET;
  private parsedCharacters = 0;
  private joinedCharacters = 0;

  get parsedTail() { return this.parsed; }
  get deferredTailRoot() { return this.root; }
  get deferredTailOpen() { return this.open; }
  get length() { return this.tailLength; }
  get parseWork() { return this.parsedCharacters; }
  get joinWork() { return this.joinedCharacters; }

  append(content: string) {
    if (!content) return;
    this.tailLength += content.length;
    this.joinedCharacters += content.length;

    let offset = 0;
    let openContent = this.open.content;
    let openId = this.open.id;
    while (offset < content.length) {
      const capacity = STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE - openContent.length;
      const take = Math.min(capacity, content.length - offset);
      openContent += content.slice(offset, offset + take);
      offset += take;
      if (openContent.length === STREAMING_MARKDOWN_DEFERRED_SLAB_SIZE) {
        const boundaryCarry = needsBoundaryCarry(openContent)
          ? openContent.slice(-1)
          : "";
        const leafContent = boundaryCarry
          ? openContent.slice(0, -1)
          : openContent;
        const leaf = Object.freeze({
          kind: "leaf" as const,
          id: openId,
          content: leafContent,
          length: leafContent.length
        });
        this.root = this.insertLeaf(this.root, leaf, this.leafCount);
        this.leafCount += 1;
        openContent = boundaryCarry;
        openId = this.nextFragmentId++;
      }
    }
    this.open = Object.freeze({ id: openId, content: openContent });
  }

  refresh(force = false) {
    if (
      this.tailLength > 0 &&
      (this.tailLength <= STREAMING_MARKDOWN_LIVE_PARSE_BUDGET ||
        this.tailLength >= this.nextParseAt ||
        force)
    ) {
      this.checkpoint();
    }
  }

  settle() {
    const content = this.materialize();
    this.parsedCharacters += this.tailLength;
    this.parsed = this.createFragment("");
    this.root = this.createBranch(STREAMING_MARKDOWN_ROPE_DEPTH);
    this.open = this.createFragment("");
    this.leafCount = 0;
    this.tailLength = 0;
    this.nextParseAt = STREAMING_MARKDOWN_LIVE_PARSE_BUDGET;
    return content;
  }

  reset() {
    this.parsed = INITIAL_PARSED;
    this.root = INITIAL_ROOT;
    this.open = INITIAL_OPEN;
    this.nextFragmentId = 3;
    this.leafCount = 0;
    this.tailLength = 0;
    this.nextParseAt = STREAMING_MARKDOWN_LIVE_PARSE_BUDGET;
    this.parsedCharacters = 0;
    this.joinedCharacters = 0;
  }

  private checkpoint() {
    this.parsed = Object.freeze({ id: this.parsed.id, content: this.materialize() });
    this.root = this.createBranch(STREAMING_MARKDOWN_ROPE_DEPTH);
    this.open = this.createFragment("");
    this.leafCount = 0;
    this.parsedCharacters += this.tailLength;
    this.nextParseAt = STREAMING_MARKDOWN_LIVE_PARSE_BUDGET;
    while (this.nextParseAt <= this.tailLength) this.nextParseAt *= 2;
  }

  private materialize() {
    const parts: string[] = [];
    if (this.parsed.content) parts.push(this.parsed.content);
    const stack: StreamingMarkdownTailNode[] = [this.root];
    while (stack.length > 0) {
      const node = stack.pop();
      if (!node) continue;
      if (node.kind === "leaf") {
        parts.push(node.content);
      } else {
        for (let index = node.children.length - 1; index >= 0; index -= 1) {
          stack.push(node.children[index]);
        }
      }
    }
    if (this.open.content) parts.push(this.open.content);
    if (parts.length <= 1) return parts[0] ?? "";
    this.joinedCharacters += this.tailLength;
    return parts.join("");
  }

  private insertLeaf(
    branch: StreamingMarkdownTailBranch,
    leaf: StreamingMarkdownTailLeaf,
    leafIndex: number
  ): StreamingMarkdownTailBranch {
    const childCapacity = STREAMING_MARKDOWN_ROPE_FANOUT ** (branch.depth - 1);
    const slot = Math.floor(leafIndex / childCapacity);
    if (slot >= STREAMING_MARKDOWN_ROPE_FANOUT) {
      throw new RangeError("streaming Markdown tail exceeds the bounded rope capacity");
    }
    const children = [...branch.children];
    if (branch.depth === 1) {
      children[slot] = leaf;
    } else {
      const existing = children[slot];
      const child =
        existing?.kind === "branch"
          ? existing
          : this.createBranch(branch.depth - 1);
      children[slot] = this.insertLeaf(
        child,
        leaf,
        leafIndex % childCapacity
      );
    }
    return Object.freeze({
      ...branch,
      children: Object.freeze(children),
      length: branch.length + leaf.length
    });
  }

  private createBranch(depth: number): StreamingMarkdownTailBranch {
    return Object.freeze({
      kind: "branch",
      id: this.nextFragmentId++,
      children: EMPTY_NODES,
      depth,
      length: 0
    });
  }

  private createFragment(content: string): StreamingMarkdownTailFragment {
    return Object.freeze({ id: this.nextFragmentId++, content });
  }
}
