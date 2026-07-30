import { memo } from "react";
import type {
  StreamingMarkdownTailFragment,
  StreamingMarkdownTailBranch,
  StreamingMarkdownTailNode
} from "./streamingMarkdownModel";

const DeferredTailNode = memo(function DeferredTailNode({
  node
}: {
  node: StreamingMarkdownTailNode;
}) {
  if (node.kind === "leaf") return <span>{node.content}</span>;
  return (
    <>
      {node.children.map((child) => (
        <DeferredTailNode key={child.id} node={child} />
      ))}
    </>
  );
});

export const StreamingMarkdownDeferredTail = memo(
  function StreamingMarkdownDeferredTail({
    root,
    open,
    fenced
  }: {
    root: StreamingMarkdownTailBranch;
    open: StreamingMarkdownTailFragment;
    fenced: boolean;
  }) {
    if (root.length === 0 && !open.content) return null;
    const content = (
      <>
        <DeferredTailNode node={root} />
        {open.content ? <span key={open.id}>{open.content}</span> : null}
      </>
    );
    return fenced ? (
      <pre className="thread-markdown-deferred-tail" data-fenced="true">
        <code>{content}</code>
      </pre>
    ) : (
      <span className="thread-markdown-deferred-tail">{content}</span>
    );
  }
);
