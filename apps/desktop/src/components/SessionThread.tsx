import {
  Bot,
  CheckCircle2,
  ChevronDown,
  Copy,
  Pencil,
  X
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  memo,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent
} from "react";
import { measureElement as measureVirtualElement, useVirtualizer } from "@tanstack/react-virtual";
import { ThinkingOrb } from "thinking-orbs";
import { getAgentSessionOutputs } from "../tauri";
import type {
  AgentOutputArtifactView,
  AgentState,
  ChatMessageView,
  TimelineEntry
} from "../tauri";
import { readSessionState, rememberSessionState } from "../sessionRuntimeModel";
import { AgentMarkdown } from "./AgentMarkdown";
import { DisclosureTriangle } from "./DisclosureTriangle";
import { ThreadOutputArtifacts, UserMessageAttachments } from "./SessionThreadArtifacts";
import {
  activeRunProgress,
  estimateThreadRowSize,
  formatThreadTime,
  LATEST_OUTPUT_THRESHOLD,
  MAX_MINIMAP_MARKERS,
  MIN_MINIMAP_MARKERS,
  RunProgressStatus,
  ThreadFind
} from "./SessionThreadNavigation";
import { SessionMinimap } from "./SessionMinimap";
import {
  EventIcon,
  ToolChainDisclosure,
  toolMessageSummary
} from "./SessionToolChain";
import { ToolActivityIcon } from "./ToolActivityIcon";
import { TraceStatusIcon } from "./TraceStatusIcon";
import {
  activeAgentActionRowId,
  associateOutputArtifacts,
  isToolRequestPlaceholder,
  sessionMinimapMarkers,
  threadMessageId,
  threadRowKey,
  updateSessionThreadProjection,
  type SessionThreadProjection,
  type SessionThreadSelection
} from "./sessionThreadProjection";
import { useSessionMinimapInteraction } from "./useSessionMinimapInteraction";
import { useModelStreamAnswer } from "./useModelStreamAnswer";
import { SessionThreadViewCache } from "./sessionThreadViewCache";
import type { StreamingMarkdownSnapshot } from "./streamingMarkdownModel";

export type { SessionThreadSelection } from "./sessionThreadProjection";

type SessionThreadProps = {
  sessionId: string | null;
  loading: boolean;
  messages: ChatMessageView[];
  timeline: TimelineEntry[];
  streamAnswer: StreamingMarkdownSnapshot | null;
  status: AgentState["status"] | "idle";
  runStartedAtMs: number;
  hasOlderHistory: boolean;
  loadingOlderHistory: boolean;
  selectedId: string | null;
  onLoadOlderHistory: () => void;
  onSelect: (selection: SessionThreadSelection) => void;
  onEditMessage: (content: string) => void;
  onArtifactInspect: (path: string) => void;
  onLinkOpenError: (message: string) => void;
};

type LiveSessionThreadProps = Omit<SessionThreadProps, "streamAnswer"> & {
  streamResetVersion: number;
  onStreamDone: (sessionId: string) => boolean | Promise<boolean>;
};

type ThreadScrollMetrics = {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
};
const SESSION_THREAD_PROJECTION_CACHE_LIMIT = 4;
const SESSION_THREAD_VIEW_CACHE_LIMIT = 4;
const SESSION_THREAD_ROW_HEIGHT_CACHE_LIMIT = 512;
const EMPTY_OUTPUT_ARTIFACTS: AgentOutputArtifactView[] = [];

function MessageIcon({ role }: { role: ChatMessageView["role"] }) {
  if (role === "tool") return <ToolActivityIcon />;
  return <Bot aria-hidden="true" />;
}

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
  onArtifactInspect,
  onLinkOpenError
}: SessionThreadProps) {
  const threadRef = useRef<HTMLElement>(null);
  const threadContentRef = useRef<HTMLDivElement>(null);
  const threadFindInputRef = useRef<HTMLInputElement>(null);
  const clipboardToastTimerRef = useRef<number | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [clipboardToast, setClipboardToast] = useState<{
    id: number;
    message: string;
    failed: boolean;
  } | null>(null);
  const [threadFindOpen, setThreadFindOpen] = useState(false);
  const [threadFindQuery, setThreadFindQuery] = useState("");
  const [threadFindIndex, setThreadFindIndex] = useState(0);
  const [arrivingMessageId, setArrivingMessageId] = useState<string | null>(null);
  const streamedAnswerRef = useRef(false);
  const scrollSyncFrameRef = useRef<number | null>(null);
  const pinLatestFrameRef = useRef<number | null>(null);
  const shortBounceAnimationRef = useRef<Animation | null>(null);
  const followLatestRef = useRef(true);
  const jumpingToLatestRef = useRef(false);
  const historyScrollIntentRef = useRef(false);
  const lastScrollTopRef = useRef(0);
  const historyLoadRequestedRef = useRef(false);
  const prependScrollHeightRef = useRef<number | null>(null);
  const projectionCacheRef = useRef<Map<string, SessionThreadProjection>>(new Map());
  const previousThreadRef = useRef<{
    sessionId: string | null;
    firstId: string | null;
    status: AgentState["status"] | "idle";
  }>({ sessionId, firstId: null, status });
  const knownMessageIdsRef = useRef<{
    sessionId: string | null;
    firstId: string | null;
    ids: Set<string>;
  }>({
    sessionId,
    firstId: messages[0] ? threadMessageId(messages[0], 0) : null,
    ids: new Set(messages.map(threadMessageId))
  });
  const [scrollMetrics, setScrollMetrics] = useState<ThreadScrollMetrics>({
    scrollTop: 0,
    scrollHeight: 1,
    clientHeight: 1
  });
  const [showJumpToLatest, setShowJumpToLatest] = useState(false);
  const [viewCache] = useState(
    () =>
      new SessionThreadViewCache(
        SESSION_THREAD_VIEW_CACHE_LIMIT,
        SESSION_THREAD_ROW_HEIGHT_CACHE_LIMIT
      )
  );
  const [artifactState, setArtifactState] = useState<{
    sessionId: string | null;
    artifacts: AgentOutputArtifactView[];
  }>({ sessionId: null, artifacts: [] });

  useLayoutEffect(() => {
    if (streamAnswer) streamedAnswerRef.current = true;
  }, [streamAnswer]);

  useLayoutEffect(() => {
    const tracker = knownMessageIdsRef.current;
    if (tracker.sessionId !== sessionId) {
      knownMessageIdsRef.current = {
        sessionId,
        firstId: messages[0] ? threadMessageId(messages[0], 0) : null,
        ids: new Set(messages.map(threadMessageId))
      };
      streamedAnswerRef.current = false;
      setArrivingMessageId(null);
      return;
    }

    const previousFirstIndex = tracker.firstId
      ? messages.findIndex(
          (message, index) => threadMessageId(message, index) === tracker.firstId
        )
      : -1;
    let nextArrival: string | null = null;
    messages.forEach((message, index) => {
      const id = threadMessageId(message, index);
      if (tracker.ids.has(id)) return;
      tracker.ids.add(id);
      if (previousFirstIndex > 0 && index < previousFirstIndex) return;
      if (message.role === "user") streamedAnswerRef.current = false;
      if (message.role === "assistant") {
        if (streamedAnswerRef.current) streamedAnswerRef.current = false;
        else nextArrival = id;
      }
    });
    tracker.firstId = messages[0] ? threadMessageId(messages[0], 0) : null;
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

  useEffect(() => {
    if (!sessionId) {
      setArtifactState({ sessionId: null, artifacts: [] });
      return;
    }
    let active = true;
    void viewCache
      .loadArtifacts(sessionId, () => getAgentSessionOutputs(sessionId))
      .then((artifacts) => {
        if (!active) return;
        setArtifactState((current) =>
          current.sessionId === sessionId && current.artifacts === artifacts
            ? current
            : { sessionId, artifacts }
        );
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [messages.length, sessionId, status, timeline.length, viewCache]);

  const outputArtifacts =
    artifactState.sessionId === sessionId
      ? artifactState.artifacts
      : sessionId
        ? viewCache.artifactsFor(sessionId) ?? EMPTY_OUTPUT_ARTIFACTS
        : EMPTY_OUTPUT_ARTIFACTS;

  const projection = useMemo(() => {
    const previous = sessionId
      ? readSessionState(projectionCacheRef.current, sessionId)
      : null;
    const next = updateSessionThreadProjection(
      previous,
      messages,
      timeline,
      MAX_MINIMAP_MARKERS
    );
    if (sessionId) {
      rememberSessionState(
        projectionCacheRef.current,
        sessionId,
        next,
        SESSION_THREAD_PROJECTION_CACHE_LIMIT
      );
    }
    return next;
  }, [messages, sessionId, timeline]);
  const { items, rows: threadRows, rowIndexByItemId } = projection;
  const editableStoppedUserMessageId = useMemo(() => {
    const latestEvent = timeline[timeline.length - 1];
    const userStoppedRun =
      status === "cancelled" &&
      (latestEvent?.label === "Agent task cancelled" ||
        latestEvent?.detail === "Agent task cancelled");
    if (!userStoppedRun) return null;
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const item = items[index];
      if (item.type === "message" && item.message.role === "user") return item.id;
    }
    return null;
  }, [items, status, timeline]);
  const { artifactsByMessageId, trailingArtifacts } = useMemo(
    () => associateOutputArtifacts(projection, outputArtifacts),
    [outputArtifacts, projection]
  );
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
  const rowVirtualizer = useVirtualizer<HTMLElement, HTMLDivElement>({
    count: threadRows.length,
    getScrollElement: () => threadRef.current,
    estimateSize: (index) => {
      const row = threadRows[index];
      const cachedHeight =
        sessionId && row ? viewCache.rowHeight(sessionId, threadRowKey(row)) : undefined;
      return cachedHeight ?? estimateThreadRowSize(row);
    },
    getItemKey: (index) => `${sessionId ?? "none"}:${threadRowKey(threadRows[index])}`,
    measureElement: (element, entry, instance) => {
      const height = measureVirtualElement(element, entry, instance);
      const index = instance.indexFromElement(element);
      const row = threadRows[index];
      if (!sessionId || !row || height <= 0 || element.dataset.sessionId !== sessionId) {
        return height;
      }
      viewCache.rememberRowHeight(sessionId, threadRowKey(row), height).forEach(
        ({ sessionId: evictedSessionId, rowKey }) => {
          instance.itemSizeCache.delete(`${evictedSessionId}:${rowKey}`);
        }
      );
      return height;
    },
    gap: 10,
    overscan: 6,
    anchorTo: "end",
    followOnAppend: "auto",
    useAnimationFrameWithResizeObserver: true
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
  const measureThreadRow = useCallback(
    (element: HTMLDivElement | null) => {
      rowVirtualizer.measureElement(element);
    },
    [rowVirtualizer]
  );
  const runProgress = useMemo(
    () => activeRunProgress(timeline, runStartedAtMs),
    [runStartedAtMs, timeline]
  );
  const activeActionRowId = useMemo(
    () => activeAgentActionRowId(threadRows, status, runStartedAtMs),
    [runStartedAtMs, status, threadRows]
  );
  const hasStreamAnswer = streamAnswer !== null;
  const runTerminal =
    status === "completed" || status === "failed" || status === "cancelled";
  const minimapMarkers = useMemo(
    () =>
      sessionMinimapMarkers(
        projection,
        streamAnswer?.preview ?? "",
        MAX_MINIMAP_MARKERS
      ),
    [projection, streamAnswer]
  );

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

  const pinLatestOutput = useCallback(
    (immediate = false) => {
      const pin = () => {
        pinLatestFrameRef.current = null;
        const thread = threadRef.current;
        if (!thread || !followLatestRef.current) return;
        thread.scrollTop = thread.scrollHeight;
        lastScrollTopRef.current = thread.scrollTop;
        setShowJumpToLatest(false);
        syncScrollMetrics();
      };

      if (immediate) {
        if (pinLatestFrameRef.current !== null) {
          window.cancelAnimationFrame(pinLatestFrameRef.current);
          pinLatestFrameRef.current = null;
        }
        pin();
        return;
      }
      if (pinLatestFrameRef.current !== null) return;
      pinLatestFrameRef.current = window.requestAnimationFrame(pin);
    },
    [syncScrollMetrics]
  );

  const pauseLatestFollow = useCallback(() => {
    jumpingToLatestRef.current = false;
    historyScrollIntentRef.current = true;
    followLatestRef.current = false;
    setShowJumpToLatest(true);
  }, []);

  const scrollToThreadFindMatch = useCallback(
    (index: number) => {
      const id = threadFindMatches[index];
      const rowIndex = id ? rowIndexByItemId.get(id) : undefined;
      if (rowIndex === undefined) return;
      pauseLatestFollow();
      rowVirtualizer.scrollToIndex(rowIndex, { align: "center" });
      window.requestAnimationFrame(syncScrollMetrics);
    },
    [pauseLatestFollow, rowIndexByItemId, rowVirtualizer, syncScrollMetrics, threadFindMatches]
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

    const requestOlderHistoryIfNeeded = () => {
      if (
        thread.clientHeight > 0 &&
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

    const handleScroll = () => {
      syncScrollMetrics();
      const currentScrollTop = thread.scrollTop;
      const historyScrollIntent = historyScrollIntentRef.current;
      historyScrollIntentRef.current = false;
      lastScrollTopRef.current = currentScrollTop;
      const distanceFromLatest = Math.max(
        0,
        thread.scrollHeight - thread.clientHeight - thread.scrollTop
      );
      const atLatest = distanceFromLatest <= LATEST_OUTPUT_THRESHOLD;
      if (jumpingToLatestRef.current) {
        followLatestRef.current = true;
        setShowJumpToLatest(false);
        if (atLatest) jumpingToLatestRef.current = false;
      } else if (historyScrollIntent) {
        followLatestRef.current = false;
        setShowJumpToLatest(thread.scrollHeight > thread.clientHeight + 2);
      } else if (!followLatestRef.current) {
        if (atLatest) {
          followLatestRef.current = true;
          setShowJumpToLatest(false);
        } else {
          setShowJumpToLatest(thread.scrollHeight > thread.clientHeight + 2);
        }
      } else if (atLatest) {
        followLatestRef.current = true;
        setShowJumpToLatest(false);
      } else if (followLatestRef.current) {
        setShowJumpToLatest(false);
        pinLatestOutput();
      } else {
        setShowJumpToLatest(thread.scrollHeight > thread.clientHeight + 2);
      }
      requestOlderHistoryIfNeeded();
    };
    const handleWheel = (event: WheelEvent) => {
      if (thread.scrollHeight <= thread.clientHeight + 2) {
        const content = threadContentRef.current;
        if (
          !content ||
          (items.length === 0 && !hasStreamAnswer) ||
          Math.abs(event.deltaY) < 1 ||
          window.matchMedia("(prefers-reduced-motion: reduce)").matches
        ) {
          return;
        }
        shortBounceAnimationRef.current?.cancel();
        const amplitude = Math.min(8, Math.max(3, Math.abs(event.deltaY) / 14));
        const offset = event.deltaY < 0 ? amplitude : -amplitude;
        shortBounceAnimationRef.current = content.animate(
          [
            { transform: "translateY(0)" },
            { transform: `translateY(${offset}px)`, offset: 0.34 },
            { transform: "translateY(0)" }
          ],
          { duration: 280, easing: "cubic-bezier(0.22, 1, 0.36, 1)" }
        );
        return;
      }
      if (event.deltaY >= 0) {
        historyScrollIntentRef.current = false;
        return;
      }
      historyScrollIntentRef.current = true;
      jumpingToLatestRef.current = false;
      followLatestRef.current = false;
      setShowJumpToLatest(true);
    };
    const handleSelectStart = () => {
      jumpingToLatestRef.current = false;
      followLatestRef.current = false;
      setShowJumpToLatest(thread.scrollHeight > thread.clientHeight + 2);
    };
    thread.addEventListener("scroll", handleScroll, { passive: true });
    thread.addEventListener("wheel", handleWheel, { passive: true });
    thread.addEventListener("selectstart", handleSelectStart, { passive: true });

    const resizeObserver = new ResizeObserver((entries) => {
      const viewportEntry = entries.find((entry) => entry.target === thread);
      const viewportResized = Boolean(viewportEntry);
      if (viewportEntry && viewCache.updateViewportWidth(viewportEntry.contentRect.width))
        rowVirtualizer.measure();
      if (followLatestRef.current && viewportResized) pinLatestOutput();
      else syncScrollMetrics();
      requestOlderHistoryIfNeeded();
    });
    resizeObserver.observe(thread);
    if (threadContentRef.current) resizeObserver.observe(threadContentRef.current);

    const frame = requestAnimationFrame(() => {
      syncScrollMetrics();
      requestOlderHistoryIfNeeded();
    });
    return () => {
      cancelAnimationFrame(frame);
      if (scrollSyncFrameRef.current !== null) {
        cancelAnimationFrame(scrollSyncFrameRef.current);
        scrollSyncFrameRef.current = null;
      }
      if (pinLatestFrameRef.current !== null) {
        cancelAnimationFrame(pinLatestFrameRef.current);
        pinLatestFrameRef.current = null;
      }
      shortBounceAnimationRef.current?.cancel();
      shortBounceAnimationRef.current = null;
      resizeObserver.disconnect();
      thread.removeEventListener("scroll", handleScroll);
      thread.removeEventListener("wheel", handleWheel);
      thread.removeEventListener("selectstart", handleSelectStart);
    };
  }, [
    hasOlderHistory,
    hasStreamAnswer,
    items.length,
    loadingOlderHistory,
    onLoadOlderHistory,
    pinLatestOutput,
    rowVirtualizer,
    syncScrollMetrics,
    viewCache
  ]);

  useEffect(() => {
    if (loadingOlderHistory) return;
    historyLoadRequestedRef.current = false;
    if (previousThreadRef.current.firstId === items[0]?.id) {
      prependScrollHeightRef.current = null;
    }
  }, [items, loadingOlderHistory]);

  useLayoutEffect(() => {
    const thread = threadRef.current;
    if (!thread) return;
    const firstId = items[0]?.id ?? null;
    const previous = previousThreadRef.current;
    const runStarted = previous.status !== "running" && status === "running";
    if (previous.sessionId !== sessionId) {
      followLatestRef.current = true;
      jumpingToLatestRef.current = false;
      historyScrollIntentRef.current = false;
      setShowJumpToLatest(false);
      pinLatestOutput(true);
    } else if (
      prependScrollHeightRef.current !== null &&
      previous.firstId !== null &&
      previous.firstId !== firstId
    ) {
      thread.scrollTop += Math.max(0, thread.scrollHeight - prependScrollHeightRef.current);
      lastScrollTopRef.current = thread.scrollTop;
      prependScrollHeightRef.current = null;
    } else if (runStarted || followLatestRef.current) {
      followLatestRef.current = true;
      setShowJumpToLatest(false);
      pinLatestOutput(runStarted);
    } else if (streamAnswer) {
      setShowJumpToLatest(thread.scrollHeight > thread.clientHeight + 2);
    }
    previousThreadRef.current = { sessionId, firstId, status };
    syncScrollMetrics();
  }, [
    items,
    outputArtifacts,
    pinLatestOutput,
    sessionId,
    status,
    streamAnswer,
    syncScrollMetrics
  ]);

  const scrollThreadToMarker = useCallback(
    (index: number) => {
      const thread = threadRef.current;
      const marker = minimapMarkers[index];
      if (!thread || !marker) return;
      if (marker.id === "streaming-answer") {
        jumpingToLatestRef.current = true;
        followLatestRef.current = true;
        historyScrollIntentRef.current = false;
        setShowJumpToLatest(false);
        thread.scrollTop = thread.scrollHeight;
      } else {
        const rowIndex = rowIndexByItemId.get(marker.id);
        if (rowIndex === undefined) return;
        pauseLatestFollow();
        rowVirtualizer.scrollToIndex(rowIndex, { align: "start" });
      }
      window.requestAnimationFrame(syncScrollMetrics);
    },
    [minimapMarkers, pauseLatestFollow, rowIndexByItemId, rowVirtualizer, syncScrollMetrics]
  );
  const {
    handlePointerCancel: handleMinimapPointerCancel,
    handlePointerDown: handleMinimapPointerDown,
    handlePointerEnd: handleMinimapPointerEnd,
    handlePointerEnter: handleMinimapPointerEnter,
    handlePointerLeave: handleMinimapPointerLeave,
    handlePointerMove: handleMinimapPointerMove,
    hoveredIndex: hoveredMinimapIndex,
    minimapRef,
    previewIndex: previewMinimapIndex
  } = useSessionMinimapInteraction(
    minimapMarkers.length,
    scrollThreadToMarker,
    scrollMetrics.scrollHeight > scrollMetrics.clientHeight
  );

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
    if (event.key === "End") {
      jumpingToLatestRef.current = true;
      followLatestRef.current = true;
      historyScrollIntentRef.current = false;
      setShowJumpToLatest(false);
    } else {
      pauseLatestFollow();
    }
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

  function jumpToLatest() {
    const thread = threadRef.current;
    if (!thread) return;
    jumpingToLatestRef.current = true;
    followLatestRef.current = true;
    historyScrollIntentRef.current = false;
    setShowJumpToLatest(false);
    thread.scrollTo({ top: thread.scrollHeight, behavior: "smooth" });
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
        data-loading={loading}
        id="session-thread-scroll"
        aria-label="Session thread"
        ref={threadRef}
      >
        {items.length === 0 && !streamAnswer && (
          <div className="session-thread-empty-state" aria-live="polite">
            {!loading && (
              <ThinkingOrb
                className="session-thread-empty-orb"
                state="solving"
                size={64}
                aria-hidden="true"
              />
            )}
            <span>{loading ? "Loading conversation" : "Cindx is ready for you."}</span>
          </div>
        )}
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
              data-session-id={sessionId ?? ""}
              key={virtualRow.key}
              ref={measureThreadRow}
              style={{ transform: `translateY(${virtualRow.start}px)` }}
            >
            {(() => {
          if (row.type === "tool-chain") {
            return (
              <ToolChainDisclosure
                key={row.id}
                row={row}
                active={row.id === activeActionRowId}
                progressLabel={row.id === activeActionRowId ? runProgress.label : undefined}
                progressDetail={row.id === activeActionRowId ? runProgress.detail : undefined}
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
          const messageSelectable = !isUser && !isAssistant;
          const messageAttachments = isUser ? item.message.attachments ?? [] : [];
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
                  <ToolActivityIcon toolName={summary.label} />
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
              role={messageSelectable ? "button" : undefined}
              tabIndex={messageSelectable ? 0 : undefined}
              onClick={messageSelectable ? () => onSelect(item) : undefined}
              onKeyDown={
                messageSelectable
                  ? (event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        onSelect(item);
                      }
                    }
                  : undefined
              }
            >
              {!isUser && !isAssistant && (
                <header>
                  <span className="thread-message-icon">
                    <MessageIcon role={item.message.role} />
                  </span>
                  <strong>{item.message.role}</strong>
                </header>
              )}
              {messageAttachments.length > 0 && (
                <UserMessageAttachments
                  attachments={messageAttachments}
                  onOpenError={onLinkOpenError}
                />
              )}
              {isAssistant ? (
                <>
                  <AgentMarkdown
                    content={item.message.content}
                    onOpenError={onLinkOpenError}
                    onCopyCode={copyCode}
                  />
                  <ThreadOutputArtifacts
                    artifacts={artifactsByMessageId.get(item.id) ?? []}
                    onInspect={onArtifactInspect}
                    onOpenError={onLinkOpenError}
                  />
                </>
              ) : item.message.content ? (
                <p>{item.message.content}</p>
              ) : !isUser ? (
                <p>Tool request</p>
              ) : null}
              {isUser && (
                <footer className="thread-message-actions">
                  <time
                    dateTime={new Date(item.message.timestampMs).toISOString()}
                    title={new Date(item.message.timestampMs).toLocaleString()}
                  >
                    {formatThreadTime(item.message.timestampMs)}
                  </time>
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
                  {editableStoppedUserMessageId === item.id ? (
                    <button
                      type="button"
                      aria-label="Edit stopped message"
                      title="Edit"
                      onClick={(event) => {
                        event.stopPropagation();
                        onEditMessage(item.message.content);
                      }}
                    >
                      <Pencil aria-hidden="true" />
                    </button>
                  ) : null}
                </footer>
              )}
            </article>
          );
            })()}
            </div>
          );
        })}
        </div>

        {trailingArtifacts.length > 0 && (
          <div className="thread-output-pending">
            <ThreadOutputArtifacts
              artifacts={trailingArtifacts}
              onInspect={onArtifactInspect}
              onOpenError={onLinkOpenError}
            />
          </div>
        )}

        {streamAnswer && !runTerminal && (
          <article
            className="thread-message thread-message-assistant thread-message-streaming"
            data-minimap-id="streaming-answer"
            data-minimap-index={items.length}
            data-minimap-kind="streaming"
          >
            <AgentMarkdown
              streamingContent={streamAnswer}
              onOpenError={onLinkOpenError}
              onCopyCode={copyCode}
            />
          </article>
        )}

        {status === "running" && !activeActionRowId && (
          <RunProgressStatus progress={runProgress} className="thread-running" />
        )}
        <span className="thread-scroll-anchor" aria-hidden="true" />
        </div>
      </section>

      {showJumpToLatest && (
        <button
          className="thread-jump-latest"
          type="button"
          aria-label="Jump to latest output"
          title="Jump to latest output"
          onClick={jumpToLatest}
        >
          <ChevronDown aria-hidden="true" />
        </button>
      )}

      <SessionMinimap
        available={minimapAvailable}
        hoveredIndex={hoveredMinimapIndex}
        markers={minimapMarkers}
        minimapRef={minimapRef}
        onKeyDown={handleMinimapKeyDown}
        onPointerCancel={handleMinimapPointerCancel}
        onPointerDown={handleMinimapPointerDown}
        onPointerEnter={handleMinimapPointerEnter}
        onPointerLeave={handleMinimapPointerLeave}
        onPointerMove={handleMinimapPointerMove}
        onPointerUp={handleMinimapPointerEnd}
        positionIndex={minimapPositionIndex}
        previewIndex={previewMinimapIndex}
        scrollRange={scrollRange}
        scrollTop={scrollMetrics.scrollTop}
      />
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

export const LiveSessionThread = memo(function LiveSessionThread({
  sessionId,
  streamResetVersion,
  onStreamDone,
  onLinkOpenError,
  ...props
}: LiveSessionThreadProps) {
  const streamAnswer = useModelStreamAnswer({
    sessionId,
    streamResetVersion,
    onStreamDone,
    onError: onLinkOpenError
  });

  return (
    <SessionThread
      {...props}
      sessionId={sessionId}
      streamAnswer={streamAnswer}
      onLinkOpenError={onLinkOpenError}
    />
  );
});
