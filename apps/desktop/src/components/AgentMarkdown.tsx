import { Copy, Maximize2 } from "lucide-react";
import {
  Children,
  isValidElement,
  memo,
  useCallback,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type ReactNode
} from "react";
import Markdown from "markdown-to-jsx";
import { openArtifact, openExternalUrl } from "../tauri";
import { DiagramFullscreen } from "./DiagramFullscreen";
import { MarkmapDiagram } from "./MarkmapDiagram";
import { MermaidDiagram } from "./MermaidDiagram";
import { StreamingMarkdownDeferredTail } from "./StreamingMarkdownDeferredTail";
import { markdownDiagramForCode } from "./markdownDiagramModel";
import type {
  StreamingMarkdownChunk,
  StreamingMarkdownSnapshot
} from "./streamingMarkdownModel";

type MarkdownLinkProps = ComponentPropsWithoutRef<"a"> & {
  onOpenError?: (message: string) => void;
};

type MarkdownCodeBlockProps = ComponentPropsWithoutRef<"pre"> & {
  onCopyCode?: (content: string) => void;
  onDiagramError?: (message: string) => void;
  renderDiagrams?: boolean;
};

type MarkdownTableProps = ComponentPropsWithoutRef<"table">;

function externalLinkTarget(href: string) {
  if (/^(https?:|mailto:)/i.test(href)) return href;
  if (/^www\./i.test(href)) return `https://${href}`;
  return null;
}

function artifactLinkTarget(href: string) {
  let target = href;
  if (/^file:\/\//i.test(target)) {
    try {
      target = new URL(target).pathname;
    } catch {
      target = target.replace(/^file:\/\//i, "");
    }
  } else {
    target = target.split(/[?#]/, 1)[0];
  }
  try {
    target = decodeURIComponent(target);
  } catch {
    // Keep the original path when a link contains a malformed escape.
  }
  return target.replace(/:\d+(?::\d+)?$/, "");
}

function MarkdownLink({
  children,
  href,
  onClick,
  onKeyDown,
  onOpenError,
  ...props
}: MarkdownLinkProps) {
  const externalTarget = href ? externalLinkTarget(href) : null;
  const reportError = (target: string, error: unknown) => {
    const detail = error instanceof Error ? error.message : String(error);
    onOpenError?.(`Could not open ${target}: ${detail}`);
  };
  return (
    <a
      {...props}
      href={href}
      target={externalTarget ? "_blank" : undefined}
      rel={externalTarget ? "noreferrer noopener" : undefined}
      onClick={(event) => {
        event.stopPropagation();
        onClick?.(event);
        if (event.defaultPrevented || !href || href.startsWith("#")) return;
        event.preventDefault();
        if (externalTarget) {
          void openExternalUrl(externalTarget).catch((error) => reportError(externalTarget, error));
          return;
        }
        const artifactTarget = artifactLinkTarget(href);
        if (artifactTarget) {
          void openArtifact(artifactTarget).catch((error) => reportError(artifactTarget, error));
        }
      }}
      onKeyDown={(event) => {
        event.stopPropagation();
        onKeyDown?.(event);
      }}
    >
      {children}
    </a>
  );
}

function markdownNodeText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(markdownNodeText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) {
    return markdownNodeText(node.props.children);
  }
  return "";
}

function MarkdownTable({ children, ...props }: MarkdownTableProps) {
  return (
    <div
      className="thread-markdown-table-shell"
      role="region"
      aria-label="Scrollable table"
      tabIndex={0}
    >
      <table {...props}>{children}</table>
    </div>
  );
}

function MarkdownCodeBlock({
  children,
  onCopyCode,
  onDiagramError,
  renderDiagrams = true,
  ...props
}: MarkdownCodeBlockProps) {
  const fullscreenTriggerRef = useRef<HTMLButtonElement>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const codeElement = Children.toArray(children).find((child) =>
    isValidElement<{ className?: string }>(child)
  );
  const codeClassName = isValidElement<{ className?: string }>(codeElement)
    ? codeElement.props.className ?? ""
    : "";
  const language = codeClassName.match(/(?:language|lang)-([^\s]+)/)?.[1] ?? "code";
  const code = markdownNodeText(children).replace(/\n$/, "");
  const diagram = renderDiagrams ? markdownDiagramForCode(language, code) : null;
  const closeFullscreen = useCallback(() => {
    setFullscreen(false);
    window.requestAnimationFrame(() =>
      fullscreenTriggerRef.current?.focus({ preventScroll: true })
    );
  }, []);

  return (
    <div className="thread-code-block">
      <header className="thread-code-block-header">
        <span>{language}</span>
        <div className="thread-code-block-actions">
          {diagram && (
            <button
              ref={fullscreenTriggerRef}
              type="button"
              aria-label={`Open ${diagram.kind === "mindmap" ? "mind map" : "Mermaid diagram"} fullscreen`}
              title="Full screen"
              onClick={(event) => {
                event.stopPropagation();
                setFullscreen(true);
              }}
            >
              <Maximize2 aria-hidden="true" />
            </button>
          )}
          <button
            type="button"
            aria-label="Copy code"
            title="Copy code"
            onClick={(event) => {
              event.stopPropagation();
              onCopyCode?.(code);
            }}
          >
            <Copy aria-hidden="true" />
          </button>
        </div>
      </header>
      {diagram?.kind === "mindmap" ? (
        <MarkmapDiagram source={diagram.source} />
      ) : diagram ? (
        <MermaidDiagram source={diagram.source} />
      ) : (
        <pre {...props}>{children}</pre>
      )}
      {fullscreen && diagram ? (
        <DiagramFullscreen
          diagram={diagram}
          onClose={closeFullscreen}
          onError={(message) => onDiagramError?.(message)}
        />
      ) : null}
    </div>
  );
}

const MarkdownChunk = memo(function MarkdownChunk({
  content,
  streaming,
  className,
  onOpenError,
  onCopyCode
}: {
  content: string;
  streaming: boolean;
  className: string;
  onOpenError: (message: string) => void;
  onCopyCode: (content: string) => void;
}) {
  return (
    <Markdown
      className={className}
      options={{
        disableParsingRawHTML: true,
        enforceAtxHeadings: true,
        forceBlock: true,
        forceWrapper: true,
        optimizeForStreaming: streaming,
        wrapper: "div",
        overrides: {
          a: {
            component: MarkdownLink,
            props: { onOpenError }
          },
          table: {
            component: MarkdownTable
          },
          pre: {
            component: MarkdownCodeBlock,
            props: { onCopyCode, onDiagramError: onOpenError, renderDiagrams: !streaming }
          }
        }
      }}
    >
      {content || "Tool request"}
    </Markdown>
  );
});

const SettledMarkdownChunks = memo(function SettledMarkdownChunks({
  chunks,
  onOpenError,
  onCopyCode
}: {
  chunks: readonly StreamingMarkdownChunk[];
  onOpenError: (message: string) => void;
  onCopyCode: (content: string) => void;
}) {
  return chunks.map((chunk) => (
    <MarkdownChunk
      key={chunk.id}
      content={chunk.content}
      streaming={false}
      className="thread-markdown-chunk"
      onOpenError={onOpenError}
      onCopyCode={onCopyCode}
    />
  ));
});

export const AgentMarkdown = memo(function AgentMarkdown({
  content,
  streamingContent,
  onOpenError,
  onCopyCode
}: {
  content?: string;
  streamingContent?: StreamingMarkdownSnapshot;
  onOpenError: (message: string) => void;
  onCopyCode: (content: string) => void;
}) {
  if (streamingContent) {
    return (
      <div className="thread-markdown thread-markdown-stream">
        <SettledMarkdownChunks
          chunks={streamingContent.settledChunks}
          onOpenError={onOpenError}
          onCopyCode={onCopyCode}
        />
        <MarkdownChunk
          key={streamingContent.tailId}
          content={streamingContent.parsedTail.content}
          streaming
          className="thread-markdown-chunk"
          onOpenError={onOpenError}
          onCopyCode={onCopyCode}
        />
        <StreamingMarkdownDeferredTail
          root={streamingContent.deferredTailRoot}
          open={streamingContent.deferredTailOpen}
          fenced={streamingContent.deferredInFence}
        />
      </div>
    );
  }
  return (
    <MarkdownChunk
      className="thread-markdown"
      content={content ?? ""}
      streaming={false}
      onOpenError={onOpenError}
      onCopyCode={onCopyCode}
    />
  );
});
