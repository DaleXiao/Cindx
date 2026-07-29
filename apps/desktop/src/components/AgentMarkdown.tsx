import { Copy, Maximize2 } from "lucide-react";
import {
  Children,
  isValidElement,
  memo,
  useCallback,
  useMemo,
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
import { markdownDiagramForCode } from "./markdownDiagramModel";

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

const STREAMING_MARKDOWN_CHUNK_TARGET = 1_600;

function splitStreamingMarkdown(content: string) {
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

export const AgentMarkdown = memo(function AgentMarkdown({
  content,
  streaming = false,
  onOpenError,
  onCopyCode
}: {
  content: string;
  streaming?: boolean;
  onOpenError: (message: string) => void;
  onCopyCode: (content: string) => void;
}) {
  const streamingChunks = useMemo(
    () => (streaming ? splitStreamingMarkdown(content) : []),
    [content, streaming]
  );
  if (streaming) {
    return (
      <div className="thread-markdown thread-markdown-stream">
        {streamingChunks.map((chunk, index) => (
          <MarkdownChunk
            key={index}
            content={chunk}
            streaming={index === streamingChunks.length - 1}
            className="thread-markdown-chunk"
            onOpenError={onOpenError}
            onCopyCode={onCopyCode}
          />
        ))}
      </div>
    );
  }
  return (
    <MarkdownChunk
      className="thread-markdown"
      content={content}
      streaming={false}
      onOpenError={onOpenError}
      onCopyCode={onCopyCode}
    />
  );
});
