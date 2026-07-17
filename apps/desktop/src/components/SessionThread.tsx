import {
  Activity,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Copy,
  FileText,
  Pencil,
  Search,
  ShieldCheck,
  TerminalSquare,
  X
} from "lucide-react";
import {
  Children,
  isValidElement,
  useCallback,
  useEffect,
  useLayoutEffect,
  memo,
  useMemo,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject
} from "react";
import Markdown from "markdown-to-jsx";
import { useVirtualizer } from "@tanstack/react-virtual";
import { openArtifact, openExternalUrl } from "../tauri";
import type { AgentState, ChatMessageView, TimelineEntry } from "../tauri";
import { DisclosureTriangle } from "./DisclosureTriangle";
import { TraceStatusIcon } from "./TraceStatusIcon";

export type SessionThreadSelection =
  | {
      id: string;
      type: "message";
      message: ChatMessageView;
    }
  | {
      id: string;
      type: "event";
      event: TimelineEntry;
    };

type SessionThreadProps = {
  sessionId: string | null;
  loading: boolean;
  messages: ChatMessageView[];
  timeline: TimelineEntry[];
  streamAnswer: string;
  status: AgentState["status"] | "idle";
  runStartedAtMs: number;
  hasOlderHistory: boolean;
  loadingOlderHistory: boolean;
  selectedId: string | null;
  onLoadOlderHistory: () => void;
  onSelect: (selection: SessionThreadSelection) => void;
  onEditMessage: (content: string) => void;
  onLinkOpenError: (message: string) => void;
};

type ThreadFindProps = {
  open: boolean;
  query: string;
  currentIndex: number;
  matchCount: number;
  inputRef: RefObject<HTMLInputElement>;
  onQueryChange: (query: string) => void;
  onMove: (direction: number) => void;
  onClose: () => void;
};

type ThreadScrollMetrics = {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
};

function threadMessageId(message: ChatMessageView, index: number) {
  return `message-${message.sequence ?? `${message.role}-${index}`}`;
}

type MinimapMarker = {
  id: string;
  kind: string;
  label: string;
  preview: string;
  targetIndex: number;
};

type ThreadRow =
  | {
      type: "item";
      item: SessionThreadSelection;
      itemIndex: number;
    }
  | {
      type: "tool-chain";
      id: string;
      items: SessionThreadSelection[];
      itemIndex: number;
    };

const MIN_MINIMAP_MARKERS = 2;
const MAX_MINIMAP_MARKERS = 32;
const MINIMAP_MARKER_GAP = 12;

function ThreadFind({
  open,
  query,
  currentIndex,
  matchCount,
  inputRef,
  onQueryChange,
  onMove,
  onClose
}: ThreadFindProps) {
  if (!open) return null;
  return (
    <div className="thread-find" role="search">
      <Search aria-hidden="true" />
      <input
        ref={inputRef}
        value={query}
        aria-label="Find in current session"
        placeholder="Find in session"
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          }
          if (event.key === "Enter") {
            event.preventDefault();
            onMove(event.shiftKey ? -1 : 1);
          }
        }}
      />
      <span aria-live="polite">
        {query ? `${matchCount === 0 ? 0 : currentIndex + 1}/${matchCount}` : ""}
      </span>
      <button
        type="button"
        disabled={matchCount === 0}
        aria-label="Previous match"
        title="Previous match"
        onClick={() => onMove(-1)}
      >
        <ChevronUp aria-hidden="true" />
      </button>
      <button
        type="button"
        disabled={matchCount === 0}
        aria-label="Next match"
        title="Next match"
        onClick={() => onMove(1)}
      >
        <ChevronDown aria-hidden="true" />
      </button>
      <button type="button" aria-label="Close find" title="Close" onClick={onClose}>
        <X aria-hidden="true" />
      </button>
    </div>
  );
}

function minimapMarkerPosition(index: number, markerCount: number) {
  const centerIndex = Math.max(0, markerCount - 1) / 2;
  const centerOffset = (index - centerIndex) * MINIMAP_MARKER_GAP;
  if (centerOffset === 0) return "50%";
  return `calc(50% ${centerOffset < 0 ? "-" : "+"} ${Math.abs(centerOffset)}px)`;
}

function activeRunProgress(timeline: TimelineEntry[], runStartedAtMs: number) {
  let startIndex = -1;
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const event = timeline[index];
    if (event.label === "Status" && /agent task started/i.test(event.detail)) {
      startIndex = index;
      break;
    }
  }
  const timelineStartedAtMs = startIndex >= 0 ? timeline[startIndex].timestampMs : 0;
  const startedAtMs = runStartedAtMs || timelineStartedAtMs || Date.now();
  let latest: TimelineEntry | undefined;
  let latestWorkflow: TimelineEntry["workflowProgress"] | undefined;
  let candidateStarts = 0;
  let candidateFinishes = 0;
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const event = timeline[index];
    if (event.timestampMs < startedAtMs) break;
    if (/^Candidate \d+$/.test(event.label)) {
      if (/started/i.test(event.detail)) candidateStarts += 1;
      if (/finished/i.test(event.detail)) candidateFinishes += 1;
    }
    if (!latestWorkflow && event.workflowProgress) latestWorkflow = event.workflowProgress;
    if (!latest && event.label !== "Message" && !/agent router selected/i.test(event.detail)) {
      latest = event;
    }
  }
  if (!latest) {
    return {
      label: "Thinking",
      detail: "Cindx is working",
      workflow: latestWorkflow ?? null
    };
  }

  let label = "Thinking";
  if (latest.label === "Tool started") {
    label = latest.detail.replace(/^Executing\s+/i, "Running ");
  } else if (latest.label === "Tool proposed") {
    label = "Preparing tool call";
  } else if (latest.label === "Tool finished") {
    label = "Processing tool result";
  } else if (latest.label === "Permission requested") {
    label = "Waiting for approval";
  } else if (latest.label === "Permission resolved") {
    label = "Resuming after approval";
  } else if (/preparing workspace knowledge/i.test(latest.detail)) {
    label = "Searching workspace knowledge";
  } else if (/preparing execution strategy/i.test(latest.detail)) {
    label = "Planning work";
  } else if (/starting execution/i.test(latest.detail)) {
    label = "Executing plan";
  } else if (/^Candidate \d+$/.test(latest.label)) {
    label = `Exploring approaches ${Math.min(candidateFinishes, candidateStarts)}/${Math.max(
      1,
      candidateStarts
    )}`;
  } else if (latest.label === "Conductor" || latest.label === "Planner") {
    label = "Planning work";
  } else if (/collaboration layer \d+\/\d+ started/i.test(latest.detail)) {
    const progress = latest.detail.match(/layer (\d+\/\d+)/i)?.[1];
    label = progress ? `Coordinating models ${progress}` : "Coordinating models";
  } else if (latest.label === "Arbiter") {
    label = "Selecting approach";
  } else if (latest.label === "Executor" || latest.label === "Model started") {
    label = "Executing plan";
  } else if (latest.label === "Reviewer") {
    label = "Reviewing result";
  } else if (latest.label === "Synthesis") {
    label = "Writing final response";
  }

  if (latestWorkflow) {
    const remaining = Math.max(0, latestWorkflow.totalSteps - latestWorkflow.completedSteps);
    const step = latestWorkflow.currentStepId
      ? ` · ${latestWorkflow.currentStepId}`
      : "";
    if (latestWorkflow.stepStatus === "failed") {
      label = "Checkpoint saved";
    } else if (/workflow resumed/i.test(latest.detail)) {
      label = "Resuming plan";
    } else if (remaining === 0) {
      label = "Finalizing plan";
    } else {
      label = "Executing plan";
    }
    latest = {
      ...latest,
      detail: `${latest.detail} · ${latestWorkflow.completedSteps}/${latestWorkflow.totalSteps} complete · ${remaining} remaining${step}${latestWorkflow.continuations ? ` · continuation ${latestWorkflow.continuations}` : ""}${latestWorkflow.recoverable ? " · checkpointed" : ""}`
    };
  }

  return { label, detail: latest.detail, workflow: latestWorkflow ?? null };
}

const RunProgressStatus = memo(function RunProgressStatus({
  progress,
  className
}: {
  progress: ReturnType<typeof activeRunProgress>;
  className: string;
}) {
  return (
    <div className={`thread-thinking ${className}`} role="status">
      <span title={progress.detail}>{progress.label}</span>
      {progress.workflow && (
        <small title={progress.detail}>
          {progress.workflow.completedSteps}/{progress.workflow.totalSteps}
          {progress.workflow.currentStepId ? ` · ${progress.workflow.currentStepId}` : ""}
        </small>
      )}
    </div>
  );
});

function EventIcon({ event }: { event: TimelineEntry }) {
  if (event.kind === "tool") return <TerminalSquare aria-hidden="true" />;
  if (event.kind === "permission") return <ShieldCheck aria-hidden="true" />;
  if (event.kind === "model") return <Activity aria-hidden="true" />;
  return <FileText aria-hidden="true" />;
}

function MessageIcon({ role }: { role: ChatMessageView["role"] }) {
  if (role === "tool") return <TerminalSquare aria-hidden="true" />;
  return <Bot aria-hidden="true" />;
}

function toolMessageSummary(content: string) {
  const tool = content.match(/^tool=(.+)$/m)?.[1]?.trim();
  const status = content.match(/^status=(.+)$/m)?.[1]?.trim();
  return {
    label: tool || "Tool output",
    status: status || "done"
  };
}

function toolMessageDetail(content: string) {
  return (
    content
      .split("\n")
      .map((line) => line.trim())
      .find((line) => line && !/^(tool|status)=/i.test(line)) ?? "Tool output"
  );
}

function isToolRequestPlaceholder(item: SessionThreadSelection) {
  const content = item.type === "message" ? item.message.content.trim().toLowerCase() : "";
  return (
    item.type === "message" &&
    item.message.role === "assistant" &&
    (!content || content === "tool request")
  );
}

function isActivityCandidate(item: SessionThreadSelection) {
  return (
    item.type === "event" ||
    (item.type === "message" && item.message.role === "tool") ||
    isToolRequestPlaceholder(item)
  );
}

function groupThreadItems(items: SessionThreadSelection[]): ThreadRow[] {
  const rows: ThreadRow[] = [];
  let index = 0;

  while (index < items.length) {
    if (!isActivityCandidate(items[index])) {
      rows.push({ type: "item", item: items[index], itemIndex: index });
      index += 1;
      continue;
    }

    let end = index + 1;
    while (end < items.length && isActivityCandidate(items[end])) end += 1;
    const candidates = items.slice(index, end);
    rows.push({
      type: "tool-chain",
      id: `tool-chain-${candidates[0].id}`,
      items: candidates,
      itemIndex: index
    });
    index = end;
  }

  return rows;
}

function threadRowKey(row: ThreadRow) {
  return row.type === "tool-chain" ? row.id : row.item.id;
}

function estimateThreadRowSize(row: ThreadRow) {
  if (row.type === "tool-chain") return 54;
  if (row.item.type === "event" || row.item.message.role === "tool") return 54;
  const content = row.item.message.content;
  const explicitLines = Math.max(1, content.split("\n").length);
  const wrappedLines = Math.max(1, Math.ceil(content.length / 72));
  const lineCount = Math.max(explicitLines, wrappedLines);
  if (row.item.message.role === "user") return 64 + Math.min(8, lineCount) * 18;
  return 48 + Math.min(48, lineCount) * 20;
}

function ToolChainItem({
  item,
  selected,
  onSelect
}: {
  item: SessionThreadSelection;
  selected: boolean;
  onSelect: (selection: SessionThreadSelection) => void;
}) {
  if (isToolRequestPlaceholder(item)) return null;

  if (item.type === "event") {
    return (
      <button
        className={`thread-tool-chain-item ${selected ? "selected" : ""}`}
        type="button"
        onClick={() => onSelect(item)}
      >
        <span className="thread-event-icon">
          <EventIcon event={item.event} />
        </span>
        <span className="thread-tool-chain-copy">
          <strong>{item.event.label}</strong>
          <small>{item.event.detail}</small>
        </span>
        <span className="thread-event-meta">
          <TraceStatusIcon status={item.event.state} />
        </span>
      </button>
    );
  }

  const summary = toolMessageSummary(item.message.content);
  return (
    <button
      className={`thread-tool-chain-item ${selected ? "selected" : ""}`}
      type="button"
      onClick={() => onSelect(item)}
    >
      <TerminalSquare aria-hidden="true" />
      <span className="thread-tool-chain-copy">
        <strong>{summary.label}</strong>
        <small>{toolMessageDetail(item.message.content)}</small>
      </span>
      <span className="thread-event-meta">
        <TraceStatusIcon status={summary.status} />
      </span>
    </button>
  );
}

const ToolChainDisclosure = memo(function ToolChainDisclosure({
  row,
  selectedId,
  onSelect
}: {
  row: Extract<ThreadRow, { type: "tool-chain" }>;
  selectedId: string | null;
  onSelect: (selection: SessionThreadSelection) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = row.items.some((item) => item.id === selectedId);

  return (
    <details
      className={`thread-tool-chain ${selected ? "selected" : ""}`}
      data-minimap-id={row.id}
      data-minimap-index={row.itemIndex}
      data-minimap-kind="tool-chain"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <strong>Agent actions</strong>
        <ChevronRight className="thread-tool-chain-chevron" aria-hidden="true" />
      </summary>
      {open && (
        <div className="thread-tool-chain-items">
          {row.items
            .filter((item) => !isToolRequestPlaceholder(item))
            .map((item) => (
              <ToolChainItem
                item={item}
                selected={selectedId === item.id}
                onSelect={onSelect}
                key={item.id}
              />
            ))}
        </div>
      )}
    </details>
  );
});

type MarkdownLinkProps = ComponentPropsWithoutRef<"a"> & {
  onOpenError?: (message: string) => void;
};

type MarkdownCodeBlockProps = ComponentPropsWithoutRef<"pre"> & {
  onCopyCode?: (content: string) => void;
};

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

function MarkdownCodeBlock({ children, onCopyCode, ...props }: MarkdownCodeBlockProps) {
  const codeElement = Children.toArray(children).find((child) =>
    isValidElement<{ className?: string }>(child)
  );
  const codeClassName = isValidElement<{ className?: string }>(codeElement)
    ? codeElement.props.className ?? ""
    : "";
  const language = codeClassName.match(/(?:language|lang)-([^\s]+)/)?.[1] ?? "code";
  const code = markdownNodeText(children).replace(/\n$/, "");

  return (
    <div className="thread-code-block">
      <header className="thread-code-block-header">
        <span>{language}</span>
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
      </header>
      <pre {...props}>{children}</pre>
    </div>
  );
}

const AgentMarkdown = memo(function AgentMarkdown({
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
  return (
    <Markdown
      className="thread-markdown"
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
          pre: {
            component: MarkdownCodeBlock,
            props: { onCopyCode }
          }
        }
      }}
    >
      {content || "Tool request"}
    </Markdown>
  );
});

export const SessionThread = memo(function SessionThread({
  sessionId,
  loading,
  messages,
  timeline,
  streamAnswer,
  status,
  runStartedAtMs,
  hasOlderHistory,
  loadingOlderHistory,
  selectedId,
  onLoadOlderHistory,
  onSelect,
  onEditMessage,
  onLinkOpenError
}: SessionThreadProps) {
  const threadRef = useRef<HTMLElement>(null);
  const threadContentRef = useRef<HTMLDivElement>(null);
  const threadFindInputRef = useRef<HTMLInputElement>(null);
  const minimapRef = useRef<HTMLDivElement>(null);
  const minimapPointerRef = useRef<number | null>(null);
  const clipboardToastTimerRef = useRef<number | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [clipboardToast, setClipboardToast] = useState<{
    id: number;
    message: string;
    failed: boolean;
  } | null>(null);
  const [hoveredMinimapIndex, setHoveredMinimapIndex] = useState<number | null>(null);
  const [previewMinimapIndex, setPreviewMinimapIndex] = useState<number | null>(null);
  const [minimapDragging, setMinimapDragging] = useState(false);
  const [threadFindOpen, setThreadFindOpen] = useState(false);
  const [threadFindQuery, setThreadFindQuery] = useState("");
  const [threadFindIndex, setThreadFindIndex] = useState(0);
  const [arrivingMessageId, setArrivingMessageId] = useState<string | null>(null);
  const streamedAnswerRef = useRef(false);
  const scrollSyncFrameRef = useRef<number | null>(null);
  const historyLoadRequestedRef = useRef(false);
  const prependScrollHeightRef = useRef<number | null>(null);
  const previousThreadRef = useRef<{ sessionId: string | null; firstId: string | null }>({
    sessionId,
    firstId: null
  });
  const knownMessageIdsRef = useRef<{ sessionId: string | null; ids: Set<string> }>({
    sessionId,
    ids: new Set(messages.map(threadMessageId))
  });
  const [scrollMetrics, setScrollMetrics] = useState<ThreadScrollMetrics>({
    scrollTop: 0,
    scrollHeight: 1,
    clientHeight: 1
  });

  useLayoutEffect(() => {
    if (streamAnswer) streamedAnswerRef.current = true;
  }, [streamAnswer]);

  useLayoutEffect(() => {
    const nextIds = new Set(messages.map(threadMessageId));
    const tracker = knownMessageIdsRef.current;
    if (tracker.sessionId !== sessionId) {
      knownMessageIdsRef.current = { sessionId, ids: nextIds };
      streamedAnswerRef.current = false;
      setArrivingMessageId(null);
      return;
    }

    let nextArrival: string | null = null;
    messages.forEach((message, index) => {
      const id = threadMessageId(message, index);
      if (tracker.ids.has(id)) return;
      if (message.role === "user") streamedAnswerRef.current = false;
      if (message.role === "assistant") {
        if (streamedAnswerRef.current) streamedAnswerRef.current = false;
        else nextArrival = id;
      }
    });
    tracker.ids = nextIds;
    if (nextArrival) setArrivingMessageId(nextArrival);
  }, [messages, sessionId]);

  useEffect(() => {
    if (!arrivingMessageId) return;
    const timeout = window.setTimeout(() => {
      setArrivingMessageId((current) => (current === arrivingMessageId ? null : current));
    }, 720);
    return () => window.clearTimeout(timeout);
  }, [arrivingMessageId]);

  useEffect(
    () => () => {
      if (clipboardToastTimerRef.current !== null) {
        window.clearTimeout(clipboardToastTimerRef.current);
      }
    },
    []
  );

  const items = useMemo<SessionThreadSelection[]>(() => {
    const messageItems = messages.map((message, index) => ({
      id: threadMessageId(message, index),
      type: "message" as const,
      message
    }));
    const eventItems = timeline
      .filter((event) => event.kind !== "message")
      .map((event, index) => ({
        id: `event-${event.sequence ?? `${event.timestampMs}-${index}`}`,
        type: "event" as const,
        event
      }));

    const merged: SessionThreadSelection[] = [];
    let messageIndex = 0;
    let eventIndex = 0;
    while (messageIndex < messageItems.length || eventIndex < eventItems.length) {
      const message = messageItems[messageIndex];
      const event = eventItems[eventIndex];
      if (message && (!event || message.message.timestampMs <= event.event.timestampMs)) {
        merged.push(message);
        messageIndex += 1;
      } else if (event) {
        merged.push(event);
        eventIndex += 1;
      }
    }
    return merged;
  }, [messages, timeline]);
  const threadFindMatches = useMemo(() => {
    const query = threadFindQuery.trim().toLocaleLowerCase();
    if (!query) return [];
    return items
      .filter(
        (item) =>
          item.type === "message" &&
          item.message.role !== "tool" &&
          !isToolRequestPlaceholder(item) &&
          item.message.content.toLocaleLowerCase().includes(query)
      )
      .map((item) => item.id);
  }, [items, threadFindQuery]);
  const threadRows = useMemo(() => groupThreadItems(items), [items]);
  const rowIndexByItemId = useMemo(() => {
    const indexes = new Map<string, number>();
    threadRows.forEach((row, rowIndex) => {
      if (row.type === "tool-chain") {
        row.items.forEach((item) => indexes.set(item.id, rowIndex));
      } else {
        indexes.set(row.item.id, rowIndex);
      }
    });
    return indexes;
  }, [threadRows]);
  const rowVirtualizer = useVirtualizer<HTMLElement, HTMLDivElement>({
    count: threadRows.length,
    getScrollElement: () => threadRef.current,
    estimateSize: (index) => estimateThreadRowSize(threadRows[index]),
    getItemKey: (index) => `${sessionId ?? "none"}:${threadRowKey(threadRows[index])}`,
    gap: 10,
    overscan: 6,
    anchorTo: "end",
    followOnAppend: "auto",
    useAnimationFrameWithResizeObserver: true
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
  const runProgress = useMemo(
    () => activeRunProgress(timeline, runStartedAtMs),
    [runStartedAtMs, timeline]
  );
  const hasStreamAnswer = Boolean(streamAnswer);
  const minimapMarkers = useMemo<MinimapMarker[]>(() => {
    const markers: MinimapMarker[] = [];
    items.forEach((item, targetIndex) => {
      if (item.type === "event") return;
      const role = item.message.role;
      if (role !== "user" && role !== "assistant" && role !== "reviewer") return;

      const preview = item.message.content.trim();
      if (!preview || preview.toLowerCase() === "tool request") return;
      markers.push({
        id: item.id,
        kind: role,
        label:
          role === "user"
            ? "You"
            : role === "assistant"
              ? "Cindx"
              : `${role.charAt(0).toUpperCase()}${role.slice(1)}`,
        preview,
        targetIndex
      });
    });

    if (streamAnswer.trim()) {
      markers.push({
        id: "streaming-answer",
        kind: "streaming",
        label: "Cindx",
        preview: streamAnswer.trim(),
        targetIndex: items.length
      });
    }

    if (markers.length <= MAX_MINIMAP_MARKERS) return markers;
    return Array.from({ length: MAX_MINIMAP_MARKERS }, (_, index) => {
      const sourceIndex = Math.round(
        (index * (markers.length - 1)) / (MAX_MINIMAP_MARKERS - 1)
      );
      return markers[sourceIndex];
    });
  }, [items, streamAnswer]);

  const syncScrollMetrics = useCallback(() => {
    if (scrollSyncFrameRef.current !== null) return;
    scrollSyncFrameRef.current = window.requestAnimationFrame(() => {
      scrollSyncFrameRef.current = null;
      const thread = threadRef.current;
      if (!thread) return;
      const nextMetrics = {
        scrollTop: Math.round(thread.scrollTop / 4) * 4,
        scrollHeight: thread.scrollHeight,
        clientHeight: thread.clientHeight
      };
      setScrollMetrics((current) =>
        current.scrollTop === nextMetrics.scrollTop &&
        current.scrollHeight === nextMetrics.scrollHeight &&
        current.clientHeight === nextMetrics.clientHeight
          ? current
          : nextMetrics
      );
    });
  }, []);

  const scrollToThreadFindMatch = useCallback(
    (index: number) => {
      const id = threadFindMatches[index];
      const rowIndex = id ? rowIndexByItemId.get(id) : undefined;
      if (rowIndex === undefined) return;
      rowVirtualizer.scrollToIndex(rowIndex, { align: "center" });
      window.requestAnimationFrame(syncScrollMetrics);
    },
    [rowIndexByItemId, rowVirtualizer, syncScrollMetrics, threadFindMatches]
  );

  const closeThreadFind = useCallback(() => {
    setThreadFindOpen(false);
    setThreadFindQuery("");
    setThreadFindIndex(0);
  }, []);

  function moveThreadFind(direction: number) {
    if (threadFindMatches.length === 0) return;
    const next =
      (threadFindIndex + direction + threadFindMatches.length) % threadFindMatches.length;
    setThreadFindIndex(next);
    scrollToThreadFindMatch(next);
  }

  useEffect(() => {
    const handleFindShortcut = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setThreadFindOpen(true);
        window.requestAnimationFrame(() => threadFindInputRef.current?.focus());
      } else if (event.key === "Escape" && threadFindOpen) {
        event.preventDefault();
        closeThreadFind();
      }
    };
    window.addEventListener("keydown", handleFindShortcut);
    return () => window.removeEventListener("keydown", handleFindShortcut);
  }, [closeThreadFind, threadFindOpen]);

  useEffect(() => {
    closeThreadFind();
  }, [closeThreadFind, sessionId]);

  useEffect(() => {
    setThreadFindIndex(0);
    if (threadFindMatches.length === 0) return;
    const frame = window.requestAnimationFrame(() => scrollToThreadFindMatch(0));
    return () => window.cancelAnimationFrame(frame);
  }, [scrollToThreadFindMatch, threadFindMatches.length]);

  useEffect(() => {
    const thread = threadRef.current;
    if (!thread) return;

    const handleScroll = () => {
      syncScrollMetrics();
      if (
        thread.scrollTop <= 160 &&
        hasOlderHistory &&
        !loadingOlderHistory &&
        !historyLoadRequestedRef.current
      ) {
        historyLoadRequestedRef.current = true;
        prependScrollHeightRef.current = thread.scrollHeight;
        onLoadOlderHistory();
      }
    };
    thread.addEventListener("scroll", handleScroll, { passive: true });

    const resizeObserver = new ResizeObserver(syncScrollMetrics);
    resizeObserver.observe(thread);
    if (threadContentRef.current) resizeObserver.observe(threadContentRef.current);

    const frame = requestAnimationFrame(syncScrollMetrics);
    return () => {
      cancelAnimationFrame(frame);
      if (scrollSyncFrameRef.current !== null) {
        cancelAnimationFrame(scrollSyncFrameRef.current);
        scrollSyncFrameRef.current = null;
      }
      resizeObserver.disconnect();
      thread.removeEventListener("scroll", handleScroll);
    };
  }, [
    hasOlderHistory,
    hasStreamAnswer,
    items.length,
    loadingOlderHistory,
    onLoadOlderHistory,
    syncScrollMetrics
  ]);

  useEffect(() => {
    if (loadingOlderHistory) return;
    historyLoadRequestedRef.current = false;
    if (previousThreadRef.current.firstId === items[0]?.id) {
      prependScrollHeightRef.current = null;
    }
  }, [items, loadingOlderHistory]);

  useEffect(() => {
    setPreviewMinimapIndex(null);
    if (hoveredMinimapIndex === null || minimapDragging) return;

    const timeout = window.setTimeout(() => {
      setPreviewMinimapIndex(hoveredMinimapIndex);
    }, 420);
    return () => window.clearTimeout(timeout);
  }, [hoveredMinimapIndex, minimapDragging]);

  useLayoutEffect(() => {
    rowVirtualizer.measure();
  }, [rowVirtualizer, sessionId]);

  useLayoutEffect(() => {
    const thread = threadRef.current;
    if (!thread) return;
    const firstId = items[0]?.id ?? null;
    const previous = previousThreadRef.current;
    if (previous.sessionId !== sessionId) {
      thread.scrollTop = thread.scrollHeight;
    } else if (
      prependScrollHeightRef.current !== null &&
      previous.firstId !== null &&
      previous.firstId !== firstId
    ) {
      thread.scrollTop += Math.max(0, thread.scrollHeight - prependScrollHeightRef.current);
      prependScrollHeightRef.current = null;
    } else {
      thread.scrollTop = thread.scrollHeight;
    }
    previousThreadRef.current = { sessionId, firstId };
    syncScrollMetrics();
  }, [items, sessionId, status, streamAnswer, syncScrollMetrics]);

  function minimapIndexFromPointer(clientY: number) {
    const minimap = minimapRef.current;
    if (!minimap || minimapMarkers.length === 0) return null;
    const bounds = minimap.getBoundingClientRect();
    const groupHeight = (minimapMarkers.length - 1) * MINIMAP_MARKER_GAP;
    const groupStart = (bounds.height - groupHeight) / 2;
    const localY = clientY - bounds.top;
    if (
      localY < groupStart - MINIMAP_MARKER_GAP / 2 ||
      localY > groupStart + groupHeight + MINIMAP_MARKER_GAP / 2
    ) {
      return null;
    }
    const index = Math.round((localY - groupStart) / MINIMAP_MARKER_GAP);
    return Math.min(minimapMarkers.length - 1, Math.max(0, index));
  }

  function scrollThreadToMarker(index: number) {
    const thread = threadRef.current;
    const marker = minimapMarkers[index];
    if (!thread || !marker) return;
    if (marker.id === "streaming-answer") {
      thread.scrollTop = thread.scrollHeight;
    } else {
      const rowIndex = rowIndexByItemId.get(marker.id);
      if (rowIndex === undefined) return;
      rowVirtualizer.scrollToIndex(rowIndex, { align: "start" });
    }
    window.requestAnimationFrame(syncScrollMetrics);
  }

  function scrollThreadToPointer(clientY: number) {
    const index = minimapIndexFromPointer(clientY);
    if (index !== null) scrollThreadToMarker(index);
  }

  function updateMinimapHover(clientY: number) {
    const index = minimapIndexFromPointer(clientY);
    setHoveredMinimapIndex((current) => (current === index ? current : index));
  }

  function handleMinimapPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (scrollMetrics.scrollHeight <= scrollMetrics.clientHeight) return;
    const markerIndex = minimapIndexFromPointer(event.clientY);
    if (markerIndex === null) return;
    event.preventDefault();
    minimapPointerRef.current = event.pointerId;
    setMinimapDragging(true);
    setPreviewMinimapIndex(null);
    setHoveredMinimapIndex(markerIndex);
    event.currentTarget.setPointerCapture(event.pointerId);
    scrollThreadToMarker(markerIndex);
  }

  function handleMinimapPointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (minimapPointerRef.current === event.pointerId) {
      scrollThreadToPointer(event.clientY);
      return;
    }
    updateMinimapHover(event.clientY);
  }

  function handleMinimapPointerEnd(event: ReactPointerEvent<HTMLDivElement>) {
    if (minimapPointerRef.current !== event.pointerId) return;
    minimapPointerRef.current = null;
    setMinimapDragging(false);
    updateMinimapHover(event.clientY);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }

  function handleMinimapPointerCancel(event: ReactPointerEvent<HTMLDivElement>) {
    if (minimapPointerRef.current === event.pointerId) {
      minimapPointerRef.current = null;
      setMinimapDragging(false);
    }
    setHoveredMinimapIndex(null);
    setPreviewMinimapIndex(null);
  }

  function handleMinimapKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const thread = threadRef.current;
    if (!thread) return;

    const pageDistance = Math.max(80, thread.clientHeight * 0.82);
    let nextTop: number | null = null;
    if (event.key === "ArrowUp") nextTop = thread.scrollTop - 56;
    if (event.key === "ArrowDown") nextTop = thread.scrollTop + 56;
    if (event.key === "PageUp") nextTop = thread.scrollTop - pageDistance;
    if (event.key === "PageDown") nextTop = thread.scrollTop + pageDistance;
    if (event.key === "Home") nextTop = 0;
    if (event.key === "End") nextTop = thread.scrollHeight;
    if (nextTop === null) return;

    event.preventDefault();
    thread.scrollTop = nextTop;
    syncScrollMetrics();
  }

  const showClipboardToast = useCallback((message: string, failed = false) => {
    if (clipboardToastTimerRef.current !== null) {
      window.clearTimeout(clipboardToastTimerRef.current);
    }
    setClipboardToast({ id: Date.now(), message, failed });
    clipboardToastTimerRef.current = window.setTimeout(() => {
      setClipboardToast(null);
      clipboardToastTimerRef.current = null;
    }, 1600);
  }, []);

  const copyContent = useCallback(async (content: string, messageId?: string) => {
    try {
      await navigator.clipboard.writeText(content);
    } catch {
      showClipboardToast("Could not copy to clipboard", true);
      return;
    }
    showClipboardToast("Copied to clipboard");
    if (!messageId) return;
    setCopiedId(messageId);
    window.setTimeout(
      () => setCopiedId((current) => (current === messageId ? null : current)),
      1400
    );
  }, [showClipboardToast]);
  const copyCode = useCallback(
    (content: string) => {
      void copyContent(content);
    },
    [copyContent]
  );

  if (items.length === 0 && !streamAnswer) {
    return (
      <div className="session-thread-shell">
        <ThreadFind
          open={threadFindOpen}
          query={threadFindQuery}
          currentIndex={threadFindIndex}
          matchCount={threadFindMatches.length}
          inputRef={threadFindInputRef}
          onQueryChange={setThreadFindQuery}
          onMove={moveThreadFind}
          onClose={closeThreadFind}
        />
        <section
          className={`session-thread session-thread-empty ${
            loading ? "session-thread-loading" : ""
          }`}
          id="session-thread-scroll"
          aria-label="Session thread"
          ref={threadRef}
        >
          <span>{loading ? "Loading conversation" : "No messages yet"}</span>
        </section>
      </div>
    );
  }

  const isScrollable = scrollMetrics.scrollHeight > scrollMetrics.clientHeight + 2;
  const minimapAvailable =
    isScrollable && minimapMarkers.length >= MIN_MINIMAP_MARKERS;
  const scrollRange = Math.max(1, scrollMetrics.scrollHeight - scrollMetrics.clientHeight);
  const scrollPosition = isScrollable ? Math.min(1, scrollMetrics.scrollTop / scrollRange) : 0;
  const minimapPositionIndex = scrollPosition * Math.max(0, minimapMarkers.length - 1);

  return (
    <div className="session-thread-shell">
      <ThreadFind
        open={threadFindOpen}
        query={threadFindQuery}
        currentIndex={threadFindIndex}
        matchCount={threadFindMatches.length}
        inputRef={threadFindInputRef}
        onQueryChange={setThreadFindQuery}
        onMove={moveThreadFind}
        onClose={closeThreadFind}
      />
      <section
        className="session-thread"
        id="session-thread-scroll"
        aria-label="Session thread"
        ref={threadRef}
      >
        <div className="thread-content" ref={threadContentRef}>
        <div
          className="thread-virtual-list"
          style={{ height: `${rowVirtualizer.getTotalSize()}px` }}
        >
        {virtualRows.map((virtualRow) => {
          const row = threadRows[virtualRow.index];
          return (
            <div
              className="thread-virtual-row"
              data-index={virtualRow.index}
              key={virtualRow.key}
              ref={rowVirtualizer.measureElement}
              style={{ transform: `translateY(${virtualRow.start}px)` }}
            >
            {(() => {
          if (row.type === "tool-chain") {
            return (
              <ToolChainDisclosure
                key={row.id}
                row={row}
                selectedId={selectedId}
                onSelect={onSelect}
              />
            );
          }

          const { item, itemIndex } = row;
          if (item.type === "event") {
            return (
              <details
                className={`thread-event-disclosure ${selectedId === item.id ? "selected" : ""}`}
                key={item.id}
                data-minimap-id={item.id}
                data-minimap-index={itemIndex}
                data-minimap-kind="event"
              >
                <summary onClick={() => onSelect(item)}>
                  <DisclosureTriangle />
                  <span className="thread-event-icon">
                    <EventIcon event={item.event} />
                  </span>
                  <span className="thread-event-copy">
                    <strong>{item.event.label}</strong>
                  </span>
                  <span className="thread-event-meta">
                    <TraceStatusIcon status={item.event.state} />
                  </span>
                </summary>
                <button
                  className="thread-event-detail"
                  type="button"
                  onClick={() => onSelect(item)}
                >
                  {item.event.detail}
                </button>
              </details>
            );
          }

          const isUser = item.message.role === "user";
          const isAssistant = item.message.role === "assistant";
          if (item.message.role === "tool") {
            const summary = toolMessageSummary(item.message.content);
            return (
              <details
                className={`thread-tool-message ${selectedId === item.id ? "selected" : ""}`}
                key={item.id}
                data-minimap-id={item.id}
                data-minimap-index={itemIndex}
                data-minimap-kind={item.message.role}
              >
                <summary onClick={() => onSelect(item)}>
                  <DisclosureTriangle />
                  <TerminalSquare aria-hidden="true" />
                  <strong>{summary.label}</strong>
                  <TraceStatusIcon status={summary.status} />
                </summary>
                <button
                  className="thread-tool-message-body"
                  type="button"
                  onClick={() => onSelect(item)}
                >
                  <pre>{item.message.content || "Tool request"}</pre>
                </button>
              </details>
            );
          }
          return (
            <article
              className={`thread-message thread-message-${item.message.role} ${
                selectedId === item.id ? "selected" : ""
              } ${
                threadFindMatches[threadFindIndex] === item.id
                  ? "thread-search-current"
                  : ""
              } ${
                isAssistant && arrivingMessageId === item.id
                  ? "thread-message-arriving"
                  : ""
              }`}
              key={item.id}
              data-minimap-id={item.id}
              data-minimap-index={itemIndex}
              data-minimap-kind={item.message.role}
              data-thread-search-id={item.id}
              role={!isUser && !isAssistant ? "button" : undefined}
              tabIndex={isUser ? undefined : 0}
              onClick={() => onSelect(item)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onSelect(item);
                }
              }}
            >
              {!isUser && !isAssistant && (
                <header>
                  <span className="thread-message-icon">
                    <MessageIcon role={item.message.role} />
                  </span>
                  <strong>{item.message.role}</strong>
                </header>
              )}
              {isAssistant ? (
                <AgentMarkdown
                  content={item.message.content}
                  onOpenError={onLinkOpenError}
                  onCopyCode={copyCode}
                />
              ) : (
                <p>{item.message.content || "Tool request"}</p>
              )}
              {isUser && (
                <footer className="thread-message-actions">
                  <button
                    type="button"
                    aria-label={copiedId === item.id ? "Message copied" : "Copy message"}
                    title={copiedId === item.id ? "Copied" : "Copy"}
                    onClick={(event) => {
                      event.stopPropagation();
                      void copyContent(item.message.content, item.id);
                    }}
                  >
                    <Copy aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    aria-label="Edit message"
                    title="Edit"
                    onClick={(event) => {
                      event.stopPropagation();
                      onEditMessage(item.message.content);
                    }}
                  >
                    <Pencil aria-hidden="true" />
                  </button>
                </footer>
              )}
            </article>
          );
            })()}
            </div>
          );
        })}
        </div>

        {streamAnswer && (
          <article
            className="thread-message thread-message-assistant thread-message-streaming"
            data-minimap-id="streaming-answer"
            data-minimap-index={items.length}
            data-minimap-kind="streaming"
          >
            <RunProgressStatus progress={runProgress} className="thread-streaming-status" />
            <AgentMarkdown
              content={streamAnswer}
              streaming
              onOpenError={onLinkOpenError}
              onCopyCode={copyCode}
            />
          </article>
        )}

        {!streamAnswer && status === "running" && (
          <RunProgressStatus progress={runProgress} className="thread-running" />
        )}
        <span className="thread-scroll-anchor" aria-hidden="true" />
        </div>
      </section>

      <div
        className="thread-minimap"
        data-scrollable={minimapAvailable}
        ref={minimapRef}
        role="scrollbar"
        aria-label="Navigate conversation"
        aria-controls="session-thread-scroll"
        aria-orientation="vertical"
        aria-valuemin={0}
        aria-valuemax={Math.round(scrollRange)}
        aria-valuenow={Math.round(scrollMetrics.scrollTop)}
        tabIndex={minimapAvailable ? 0 : -1}
        onKeyDown={handleMinimapKeyDown}
        onPointerEnter={(event) => updateMinimapHover(event.clientY)}
        onPointerDown={handleMinimapPointerDown}
        onPointerMove={handleMinimapPointerMove}
        onPointerUp={handleMinimapPointerEnd}
        onPointerCancel={handleMinimapPointerCancel}
        onPointerLeave={() => {
          if (minimapPointerRef.current !== null) return;
          setHoveredMinimapIndex(null);
          setPreviewMinimapIndex(null);
        }}
      >
        <div className="thread-minimap-track" aria-hidden="true">
          {minimapMarkers.map((marker, index) => {
            const waveDistance =
              hoveredMinimapIndex === null ? null : Math.abs(index - hoveredMinimapIndex);
            return (
              <span
                className={`thread-minimap-marker thread-minimap-marker-${marker.kind}`}
                data-wave-distance={
                  waveDistance !== null && waveDistance <= 2 ? waveDistance : undefined
                }
                data-edge-fade={index < 3 ? index : undefined}
                key={marker.id}
                style={{ top: minimapMarkerPosition(index, minimapMarkers.length) }}
              />
            );
          })}
          <span
            className="thread-minimap-position"
            style={{
              top: minimapMarkerPosition(minimapPositionIndex, minimapMarkers.length)
            }}
          />
        </div>
        {previewMinimapIndex !== null && minimapMarkers[previewMinimapIndex] && (
          <aside
            className="thread-minimap-preview"
            role="tooltip"
            style={{
              top: `clamp(48px, ${minimapMarkerPosition(
                previewMinimapIndex,
                minimapMarkers.length
              )}, calc(100% - 48px))`
            }}
          >
            <header>
              <strong>{minimapMarkers[previewMinimapIndex].label}</strong>
            </header>
            <p>{minimapMarkers[previewMinimapIndex].preview}</p>
          </aside>
        )}
      </div>
      {clipboardToast && (
        <div
          className="clipboard-toast"
          data-failed={clipboardToast.failed}
          key={clipboardToast.id}
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          {clipboardToast.failed ? (
            <X aria-hidden="true" />
          ) : (
            <CheckCircle2 aria-hidden="true" />
          )}
          <span>{clipboardToast.message}</span>
        </div>
      )}
    </div>
  );
});
