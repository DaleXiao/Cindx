import {
  Activity,
  Bot,
  Copy,
  FileText,
  Pencil,
  ShieldCheck,
  TerminalSquare
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from "react";
import Markdown from "markdown-to-jsx";
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
  messages: ChatMessageView[];
  timeline: TimelineEntry[];
  streamAnswer: string;
  status: AgentState["status"] | "idle";
  selectedId: string | null;
  onSelect: (selection: SessionThreadSelection) => void;
  onEditMessage: (content: string) => void;
};

type ThreadScrollMetrics = {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
};

type MinimapMarker = {
  id: string;
  kind: string;
  label: string;
  time: string;
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
const MINIMAP_MARKER_GAP = 14;
const MINIMAP_MARKER_START = 8;

function minimapMarkerPosition(index: number) {
  return `${MINIMAP_MARKER_START + index * MINIMAP_MARKER_GAP}px`;
}

function formatThreadTime(timestampMs: number) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit"
  }).format(timestampMs);
}

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
  return (
    item.type === "message" &&
    item.message.role === "assistant" &&
    item.message.content.trim().toLowerCase() === "tool request"
  );
}

function isActivityCandidate(item: SessionThreadSelection) {
  return (
    item.type === "event" ||
    (item.type === "message" && item.message.role === "tool") ||
    isToolRequestPlaceholder(item)
  );
}

function containsToolActivity(items: SessionThreadSelection[]) {
  return items.some(
    (item) =>
      (item.type === "event" && ["tool", "permission"].includes(item.event.kind)) ||
      (item.type === "message" && item.message.role === "tool")
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
    if (containsToolActivity(candidates)) {
      rows.push({
        type: "tool-chain",
        id: `tool-chain-${candidates[0].id}`,
        items: candidates,
        itemIndex: index
      });
    } else {
      candidates.forEach((item, offset) => {
        rows.push({ type: "item", item, itemIndex: index + offset });
      });
    }
    index = end;
  }

  return rows;
}

function threadItemTimestamp(item: SessionThreadSelection) {
  return item.type === "event" ? item.event.timestampMs : item.message.timestampMs;
}

function toolChainStatus(items: SessionThreadSelection[]) {
  const statuses = items.flatMap((item) => {
    if (item.type === "event") return [item.event.state.toLowerCase()];
    if (item.message.role === "tool") return [toolMessageSummary(item.message.content).status];
    return [];
  });
  if (statuses.some((status) => ["failed", "error", "cancelled", "denied"].includes(status))) {
    return "failed";
  }
  if (
    statuses.length > 0 &&
    statuses.every((status) => ["done", "completed", "succeeded"].includes(status))
  ) {
    return "done";
  }
  if (statuses.some((status) => ["pending", "waiting"].includes(status))) return "waiting";
  return "running";
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
          <time>{formatThreadTime(item.event.timestampMs)}</time>
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
        <time>{formatThreadTime(item.message.timestampMs)}</time>
      </span>
    </button>
  );
}

function MarkdownLink({
  children,
  href,
  onClick,
  onKeyDown,
  ...props
}: ComponentPropsWithoutRef<"a">) {
  const opensExternally = Boolean(href && /^(https?:|mailto:)/i.test(href));
  return (
    <a
      {...props}
      href={href}
      target={opensExternally ? "_blank" : undefined}
      rel={opensExternally ? "noreferrer noopener" : undefined}
      onClick={(event) => {
        event.stopPropagation();
        onClick?.(event);
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

function AgentMarkdown({ content, streaming = false }: { content: string; streaming?: boolean }) {
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
            component: MarkdownLink
          }
        }
      }}
    >
      {content || "Tool request"}
    </Markdown>
  );
}

export function SessionThread({
  messages,
  timeline,
  streamAnswer,
  status,
  selectedId,
  onSelect,
  onEditMessage
}: SessionThreadProps) {
  const threadRef = useRef<HTMLElement>(null);
  const minimapRef = useRef<HTMLDivElement>(null);
  const minimapPointerRef = useRef<number | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [hoveredMinimapIndex, setHoveredMinimapIndex] = useState<number | null>(null);
  const [previewMinimapIndex, setPreviewMinimapIndex] = useState<number | null>(null);
  const [minimapDragging, setMinimapDragging] = useState(false);
  const [scrollMetrics, setScrollMetrics] = useState<ThreadScrollMetrics>({
    scrollTop: 0,
    scrollHeight: 1,
    clientHeight: 1
  });
  const items = useMemo<SessionThreadSelection[]>(() => {
    const messageItems = messages.map((message, index) => ({
      id: `message-${message.timestampMs}-${index}`,
      type: "message" as const,
      message
    }));
    const eventItems = timeline
      .filter((event) => event.kind !== "message")
      .map((event, index) => ({
        id: `event-${event.timestampMs}-${index}`,
        type: "event" as const,
        event
      }));

    return [...messageItems, ...eventItems].sort((left, right) => {
      const leftTimestamp = left.type === "message" ? left.message.timestampMs : left.event.timestampMs;
      const rightTimestamp =
        right.type === "message" ? right.message.timestampMs : right.event.timestampMs;
      return leftTimestamp - rightTimestamp;
    });
  }, [messages, timeline]);
  const threadRows = useMemo(() => groupThreadItems(items), [items]);
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
        time: formatThreadTime(item.message.timestampMs),
        preview,
        targetIndex
      });
    });

    if (streamAnswer.trim()) {
      markers.push({
        id: "streaming-answer",
        kind: "streaming",
        label: "Cindx",
        time: "Thinking",
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
    const thread = threadRef.current;
    if (!thread) return;

    const nextMetrics = {
      scrollTop: thread.scrollTop,
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
  }, []);

  useEffect(() => {
    const thread = threadRef.current;
    if (!thread) return;

    const handleScroll = () => syncScrollMetrics();
    thread.addEventListener("scroll", handleScroll, { passive: true });

    const resizeObserver = new ResizeObserver(syncScrollMetrics);
    resizeObserver.observe(thread);
    thread
      .querySelectorAll<HTMLElement>("[data-minimap-kind]")
      .forEach((node) => resizeObserver.observe(node));

    const frame = requestAnimationFrame(syncScrollMetrics);
    return () => {
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      thread.removeEventListener("scroll", handleScroll);
    };
  }, [hasStreamAnswer, items.length, syncScrollMetrics]);

  useEffect(() => {
    setPreviewMinimapIndex(null);
    if (hoveredMinimapIndex === null || minimapDragging) return;

    const timeout = window.setTimeout(() => {
      setPreviewMinimapIndex(hoveredMinimapIndex);
    }, 420);
    return () => window.clearTimeout(timeout);
  }, [hoveredMinimapIndex, minimapDragging]);

  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const thread = threadRef.current;
      if (thread) {
        thread.scrollTop = thread.scrollHeight;
        syncScrollMetrics();
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [items.length, status, streamAnswer, syncScrollMetrics]);

  function minimapIndexFromPointer(clientY: number) {
    const minimap = minimapRef.current;
    if (!minimap || minimapMarkers.length === 0) return null;
    const bounds = minimap.getBoundingClientRect();
    const groupHeight = (minimapMarkers.length - 1) * MINIMAP_MARKER_GAP;
    const groupStart = MINIMAP_MARKER_START;
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
    const markerNode = thread.querySelector<HTMLElement>(
      `[data-minimap-index="${marker.targetIndex}"]`
    );
    if (!markerNode) return;
    thread.scrollTop = Math.max(0, markerNode.offsetTop - 18);
    syncScrollMetrics();
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

  async function copyMessage(id: string, content: string) {
    try {
      await navigator.clipboard.writeText(content);
    } catch {
      return;
    }
    setCopiedId(id);
    window.setTimeout(() => setCopiedId((current) => (current === id ? null : current)), 1400);
  }

  if (items.length === 0 && !streamAnswer) {
    return (
      <div className="session-thread-shell">
        <section
          className="session-thread session-thread-empty"
          id="session-thread-scroll"
          aria-label="Session thread"
          ref={threadRef}
        >
          <span>No messages yet</span>
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
      <section
        className="session-thread"
        id="session-thread-scroll"
        aria-label="Session thread"
        ref={threadRef}
      >
        {threadRows.map((row) => {
          if (row.type === "tool-chain") {
            const visibleItems = row.items.filter((item) => !isToolRequestPlaceholder(item));
            const latestTimestamp = Math.max(...row.items.map(threadItemTimestamp));
            return (
              <details
                className={`thread-tool-chain ${
                  row.items.some((item) => item.id === selectedId) ? "selected" : ""
                }`}
                key={row.id}
                data-minimap-id={row.id}
                data-minimap-index={row.itemIndex}
                data-minimap-kind="tool-chain"
              >
                <summary>
                  <DisclosureTriangle />
                  <TerminalSquare aria-hidden="true" />
                  <strong>Tool activity</strong>
                  <TraceStatusIcon status={toolChainStatus(row.items)} />
                  <time>{formatThreadTime(latestTimestamp)}</time>
                </summary>
                <div className="thread-tool-chain-items">
                  {visibleItems.map((item) => (
                    <ToolChainItem
                      item={item}
                      selected={selectedId === item.id}
                      onSelect={onSelect}
                      key={item.id}
                    />
                  ))}
                </div>
              </details>
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
                    <time>{formatThreadTime(item.event.timestampMs)}</time>
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
                  <time>{formatThreadTime(item.message.timestampMs)}</time>
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
              }`}
              key={item.id}
              data-minimap-id={item.id}
              data-minimap-index={itemIndex}
              data-minimap-kind={item.message.role}
              role={isUser ? undefined : "button"}
              tabIndex={0}
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
                  <time>{formatThreadTime(item.message.timestampMs)}</time>
                </header>
              )}
              {isAssistant ? (
                <AgentMarkdown content={item.message.content} />
              ) : (
                <p>{item.message.content || "Tool request"}</p>
              )}
              {isAssistant && (
                <footer className="thread-message-agent-meta">
                  <time>{formatThreadTime(item.message.timestampMs)}</time>
                </footer>
              )}
              {isUser && (
                <footer className="thread-message-actions">
                  <time>{formatThreadTime(item.message.timestampMs)}</time>
                  <button
                    type="button"
                    aria-label={copiedId === item.id ? "Message copied" : "Copy message"}
                    title={copiedId === item.id ? "Copied" : "Copy"}
                    onClick={(event) => {
                      event.stopPropagation();
                      void copyMessage(item.id, item.message.content);
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
        })}

        {streamAnswer && (
          <article
            className="thread-message thread-message-assistant thread-message-streaming"
            data-minimap-id="streaming-answer"
            data-minimap-index={items.length}
            data-minimap-kind="streaming"
          >
            <div className="thread-thinking thread-streaming-status" role="status">
              <span>Thinking</span>
            </div>
            <AgentMarkdown content={streamAnswer} streaming />
          </article>
        )}

        {!streamAnswer && status === "running" && (
          <div className="thread-thinking thread-running" role="status">
            <span>Thinking</span>
          </div>
        )}
        <span className="thread-scroll-anchor" aria-hidden="true" />
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
                style={{ top: minimapMarkerPosition(index) }}
              />
            );
          })}
          <span
            className="thread-minimap-position"
            style={{ top: minimapMarkerPosition(minimapPositionIndex) }}
          />
        </div>
        {previewMinimapIndex !== null && minimapMarkers[previewMinimapIndex] && (
          <aside
            className="thread-minimap-preview"
            role="tooltip"
            style={{
              top: `clamp(48px, ${minimapMarkerPosition(
                previewMinimapIndex
              )}, calc(100% - 48px))`
            }}
          >
            <header>
              <strong>{minimapMarkers[previewMinimapIndex].label}</strong>
              <time>{minimapMarkers[previewMinimapIndex].time}</time>
            </header>
            <p>{minimapMarkers[previewMinimapIndex].preview}</p>
          </aside>
        )}
      </div>
    </div>
  );
}
