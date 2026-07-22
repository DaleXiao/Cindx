import {
  Activity,
  Bug,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Clock3,
  Copy,
  Database,
  ExternalLink,
  File,
  FileText,
  FolderOpen,
  Globe2,
  Image,
  Maximize2,
  Minimize2,
  PackageOpen,
  Save,
  ShieldCheck,
  TerminalSquare,
  TriangleAlert,
  X
} from "lucide-react";
import Markdown from "markdown-to-jsx";
import { createPortal } from "react-dom";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent
} from "react";
import {
  getAgentSessionOutputs,
  openArtifact,
  readArtifactPreview,
  revealArtifact
} from "../tauri";
import type {
  AgentOutputArtifactView,
  AgentState,
  AgentTraceRoleSummary,
  AgentTraceStepView,
  ArtifactPreview,
  BrowserObservationView,
  ContextCheckpointView,
  RagSourceView,
  ToolRunView
} from "../tauri";
import type { SessionThreadSelection } from "./SessionThread";
import { TraceStatusIcon } from "./TraceStatusIcon";

export type InspectorTab = "trace" | "details" | "artifacts" | "context";

type ReviewCounts = {
  agent: number;
  tool: number;
  browser: number;
};

type InspectorProps = {
  open: boolean;
  showDebug: boolean;
  width: number;
  tab: InspectorTab;
  sessionId: string | null;
  outputRequest: { sessionId: string; path: string; nonce: number } | null;
  threadSelection: SessionThreadSelection | null;
  traceStep: AgentTraceStepView | null;
  traceExportPath: string | null;
  contextCheckpoint: ContextCheckpointView | null;
  ragAnswer: string | null;
  ragSources: RagSourceView[];
  browserObservations: BrowserObservationView[];
  toolResults: ToolRunView[];
  sessionTraceSteps: AgentTraceStepView[];
  roleSummaries: AgentTraceRoleSummary[];
  workspaceRoot: string;
  agentStatus: AgentState["status"] | "idle";
  agentTurnCount: number;
  agentMaxTurns: number;
  reviewCounts: ReviewCounts;
  onTabChange: (tab: InspectorTab) => void;
  onTraceStepSelect: (stepId: string) => void;
  onTraceExport: () => void;
  traceBusy: boolean;
  onWidthChange: (width: number) => void;
  onResizeStart: () => void;
  onResizeEnd: () => void;
  onReview: () => void;
};

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "local";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(timestampMs);
}

function formatDuration(durationMs: number | null) {
  if (durationMs == null) return "live";
  if (durationMs < 1000) return `${durationMs} ms`;
  return `${(durationMs / 1000).toFixed(1)} s`;
}

function clampWidth(width: number) {
  return Math.min(520, Math.max(280, width));
}

type OutputArtifact = AgentOutputArtifactView & {
  versionCount: number;
};

const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "jpeg", "jpg", "png", "webp"]);
const MARKDOWN_EXTENSIONS = new Set(["md", "mdown", "markdown"]);
const HTML_EXTENSIONS = new Set(["htm", "html"]);
const OUTPUT_HISTORY_SESSION_LIMIT = 12;
const DEBUG_CLOSE_ANIMATION_MS = 230;

function artifactKind(path: string): AgentOutputArtifactView["kind"] {
  return IMAGE_EXTENSIONS.has(artifactExtension(path)) ? "image" : "file";
}

function rememberOutputHistory(
  current: Record<string, OutputArtifact[]>,
  sessionId: string,
  artifacts: OutputArtifact[]
) {
  const next = { ...current };
  delete next[sessionId];
  next[sessionId] = artifacts;
  const sessionIds = Object.keys(next);
  while (sessionIds.length > OUTPUT_HISTORY_SESSION_LIMIT) {
    const oldestSessionId = sessionIds.shift();
    if (oldestSessionId) delete next[oldestSessionId];
  }
  return next;
}

function artifactName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function artifactExtension(path: string) {
  const parts = path.split(".");
  return parts[parts.length - 1]?.toLowerCase() ?? "";
}

function absoluteArtifactPath(workspaceRoot: string, path: string) {
  if (path.startsWith("/")) return path;
  if (!workspaceRoot) return path;
  return `${workspaceRoot.replace(/\/$/, "")}/${path.replace(/^\.\//, "")}`;
}

function isInternalRuntimePath(path: string) {
  return path.split(/[\\/]/).some((component) => component === ".cindx");
}

function sessionArtifact(step: AgentTraceStepView) {
  const sourcePath =
    step.metadata.result_source_path ??
    (step.toolName === "file.write" ? step.metadata.result_path ?? null : null);
  const path =
    step.metadata.result_artifact_path ??
    step.artifactPath ??
    (step.toolName === "file.write" ? step.metadata.result_path ?? null : null);
  if (!path?.trim() || isInternalRuntimePath(sourcePath ?? path)) return null;
  return { path, sourcePath };
}

function resolveOutputArtifact(
  workspaceRoot: string,
  artifact: AgentOutputArtifactView
): OutputArtifact {
  return {
    ...artifact,
    path: absoluteArtifactPath(workspaceRoot, artifact.path),
    sourcePath: artifact.sourcePath
      ? absoluteArtifactPath(workspaceRoot, artifact.sourcePath)
      : null,
    versionCount: 1
  };
}

function traceOutputArtifacts(
  sessionTraceSteps: AgentTraceStepView[],
  workspaceRoot: string
) {
  return sessionTraceSteps.flatMap((step) => {
    if (step.status === "failed") return [];
    const artifact = sessionArtifact(step);
    if (!artifact) return [];
    return [
      resolveOutputArtifact(workspaceRoot, {
        id: `${step.id}-0`,
        path: artifact.path,
        sourcePath: artifact.sourcePath,
        toolName: step.toolName ?? step.label,
        status: step.status,
        timestampMs: step.finishedAtMs ?? step.startedAtMs,
        runId: step.metadata.agent_run_id ?? null,
        version: 0,
        kind: artifactKind(artifact.path)
      })
    ];
  });
}

function mergeOutputArtifacts(...groups: OutputArtifact[][]) {
  const merged = new Map<string, OutputArtifact>();
  groups.flat().forEach((artifact) => {
    const logicalPath = artifact.sourcePath ?? artifact.path;
    const immutableVersion = Boolean(
      artifact.sourcePath && artifact.sourcePath !== artifact.path
    );
    const key = immutableVersion ? `version:${artifact.path}` : `current:${logicalPath}`;
    const current = merged.get(key);
    if (!current || artifact.timestampMs >= current.timestampMs) {
      merged.set(key, artifact);
    }
  });

  const versionedSources = new Set(
    [...merged.values()]
      .filter((artifact) => artifact.sourcePath && artifact.sourcePath !== artifact.path)
      .map((artifact) => artifact.sourcePath as string)
  );
  const chronological = [...merged.values()]
    .filter((artifact) => {
      const logicalPath = artifact.sourcePath ?? artifact.path;
      return artifact.sourcePath !== artifact.path || !versionedSources.has(logicalPath);
    })
    .sort(
      (left, right) =>
        left.timestampMs - right.timestampMs || left.id.localeCompare(right.id)
    );
  const versionBySource = new Map<string, number>();
  const countBySource = new Map<string, number>();
  chronological.forEach((artifact) => {
    const logicalPath = artifact.sourcePath ?? artifact.path;
    countBySource.set(logicalPath, (countBySource.get(logicalPath) ?? 0) + 1);
  });

  return chronological
    .map((artifact) => {
      const logicalPath = artifact.sourcePath ?? artifact.path;
      const version = (versionBySource.get(logicalPath) ?? 0) + 1;
      versionBySource.set(logicalPath, version);
      return {
        ...artifact,
        version,
        versionCount: countBySource.get(logicalPath) ?? 1
      };
    })
    .sort(
      (left, right) =>
        right.timestampMs - left.timestampMs || right.id.localeCompare(left.id)
    );
}

function outputArtifactsUnchanged(left: OutputArtifact[], right: OutputArtifact[]) {
  if (left.length !== right.length) return false;
  return left.every((artifact, index) => {
    const next = right[index];
    return (
      artifact.id === next?.id &&
      artifact.path === next.path &&
      artifact.timestampMs === next.timestampMs &&
      artifact.version === next.version
    );
  });
}

function outputDisplayPath(artifact: OutputArtifact) {
  return artifact.sourcePath ?? artifact.path;
}

function ArtifactTypeIcon({ path }: { path: string }) {
  const extension = artifactExtension(path);
  if (IMAGE_EXTENSIONS.has(extension)) return <Image aria-hidden="true" />;
  if (MARKDOWN_EXTENSIONS.has(extension)) return <FileText aria-hidden="true" />;
  if (HTML_EXTENSIONS.has(extension)) return <Globe2 aria-hidden="true" />;
  return <File aria-hidden="true" />;
}

function TraceIcon({ step }: { step: AgentTraceStepView }) {
  if (step.kind === "tool") return <TerminalSquare aria-hidden="true" />;
  if (step.kind === "permission") return <ShieldCheck aria-hidden="true" />;
  if (step.kind === "model") return <Activity aria-hidden="true" />;
  if (step.kind === "error") return <TriangleAlert aria-hidden="true" />;
  return <FileText aria-hidden="true" />;
}

function ArtifactPreviewPane({ path }: { path: string }) {
  const [preview, setPreview] = useState<ArtifactPreview | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setError(null);
    void readArtifactPreview(path)
      .then((next) => {
        if (active) setPreview(next);
      })
      .catch((reason) => {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      active = false;
    };
  }, [path]);

  if (error) return <div className="inspector-preview-message">{error}</div>;
  if (!preview) return <div className="inspector-preview-message">Loading preview</div>;
  if (preview.kind === "image" && preview.dataUrl) {
    return <img className="inspector-preview-image" src={preview.dataUrl} alt={artifactName(path)} />;
  }
  if (preview.kind === "html" && preview.content != null) {
    return (
      <iframe
        className="inspector-preview-frame"
        title={`Preview ${artifactName(path)}`}
        sandbox=""
        srcDoc={preview.content}
      />
    );
  }
  if (preview.kind === "markdown" && preview.content != null) {
    return <Markdown className="thread-markdown inspector-markdown-preview">{preview.content}</Markdown>;
  }
  if (preview.kind === "text" && preview.content != null) {
    return <pre className="inspector-text-preview">{preview.content}</pre>;
  }
  return <div className="inspector-preview-message">Preview unavailable for this file type</div>;
}

export function Inspector({
  open,
  showDebug,
  width,
  tab,
  sessionId,
  outputRequest,
  threadSelection,
  traceStep,
  traceExportPath,
  contextCheckpoint,
  ragAnswer,
  ragSources,
  browserObservations,
  toolResults,
  sessionTraceSteps,
  roleSummaries,
  workspaceRoot,
  agentStatus,
  agentTurnCount,
  agentMaxTurns,
  reviewCounts,
  onTabChange,
  onTraceStepSelect,
  onTraceExport,
  traceBusy,
  onWidthChange,
  onResizeStart,
  onResizeEnd,
  onReview
}: InspectorProps) {
  const [debugOpen, setDebugOpen] = useState(false);
  const [debugBodyMounted, setDebugBodyMounted] = useState(false);
  const [outputsOpen, setOutputsOpen] = useState(true);
  const [metadataOpen, setMetadataOpen] = useState(false);
  const [metadataMotion, setMetadataMotion] = useState<"idle" | "opening" | "closing">(
    "idle"
  );
  const [selectedOutputPath, setSelectedOutputPath] = useState<string | null>(null);
  const [outputPreviewFullscreen, setOutputPreviewFullscreen] = useState(false);
  const [openingOutputPath, setOpeningOutputPath] = useState<string | null>(null);
  const [outputActionError, setOutputActionError] = useState<string | null>(null);
  const [outputHistoryBySession, setOutputHistoryBySession] = useState<
    Record<string, OutputArtifact[]>
  >({});
  const [sessionCopyState, setSessionCopyState] = useState<
    "idle" | "copied" | "failed"
  >("idle");
  const sessionCopyTimerRef = useRef<number | null>(null);
  const debugUnmountTimerRef = useRef<number | null>(null);
  const debugOpenFrameRef = useRef<number | null>(null);
  const debugDesiredOpenRef = useRef(false);
  const debugBodyRef = useRef<HTMLDivElement | null>(null);
  const debugToggleRef = useRef<HTMLButtonElement | null>(null);
  const reviewTotal = reviewCounts.agent + reviewCounts.tool + reviewCounts.browser;
  const hasArtifacts = Boolean(ragAnswer || ragSources.length || browserObservations.length || toolResults.length);
  const hasContext = Boolean(
    contextCheckpoint && (contextCheckpoint.eventCount > 0 || contextCheckpoint.path)
  );
  const traceSummary = useMemo(() => {
    if (!debugBodyMounted) {
      return { turnCount: 0, toolCallCount: 0, permissionCount: 0 };
    }
    const turns = new Set<number>();
    let toolCallCount = 0;
    let permissionCount = 0;
    for (const step of sessionTraceSteps) {
      turns.add(step.turnIndex);
      if (step.kind === "tool") toolCallCount += 1;
      if (step.kind === "permission") permissionCount += 1;
    }
    return { turnCount: turns.size, toolCallCount, permissionCount };
  }, [debugBodyMounted, sessionTraceSteps]);
  const currentRunOutputs = useMemo(
    () => traceOutputArtifacts(sessionTraceSteps, workspaceRoot),
    [sessionTraceSteps, workspaceRoot]
  );
  const historicalOutputs = sessionId ? outputHistoryBySession[sessionId] ?? [] : [];
  const outputArtifacts = useMemo(
    () => mergeOutputArtifacts(historicalOutputs, currentRunOutputs),
    [currentRunOutputs, historicalOutputs]
  );

  useEffect(() => {
    if (!open || !sessionId) return;
    let active = true;
    void getAgentSessionOutputs(sessionId)
      .then((artifacts) => {
        if (!active) return;
        const resolved = artifacts.map((artifact) =>
          resolveOutputArtifact(workspaceRoot, artifact)
        );
        setOutputHistoryBySession((current) => {
          const authoritative = mergeOutputArtifacts(resolved);
          if (outputArtifactsUnchanged(current[sessionId] ?? [], authoritative)) return current;
          return rememberOutputHistory(current, sessionId, authoritative);
        });
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [open, sessionId, workspaceRoot]);

  useEffect(() => {
    if (!sessionId || currentRunOutputs.length === 0) return;
    setOutputHistoryBySession((current) => {
      const merged = mergeOutputArtifacts(current[sessionId] ?? [], currentRunOutputs);
      if (outputArtifactsUnchanged(current[sessionId] ?? [], merged)) return current;
      return rememberOutputHistory(current, sessionId, merged);
    });
  }, [currentRunOutputs, sessionId]);

  useEffect(() => {
    if (showDebug) return;
    debugDesiredOpenRef.current = false;
    if (debugUnmountTimerRef.current !== null) {
      window.clearTimeout(debugUnmountTimerRef.current);
      debugUnmountTimerRef.current = null;
    }
    if (debugOpenFrameRef.current !== null) {
      window.cancelAnimationFrame(debugOpenFrameRef.current);
      debugOpenFrameRef.current = null;
    }
    setDebugOpen(false);
    setDebugBodyMounted(false);
  }, [showDebug]);

  useEffect(
    () => () => {
      if (debugUnmountTimerRef.current !== null) {
        window.clearTimeout(debugUnmountTimerRef.current);
      }
      if (debugOpenFrameRef.current !== null) {
        window.cancelAnimationFrame(debugOpenFrameRef.current);
      }
    },
    []
  );

  function setDebugVisibility(nextOpen: boolean) {
    debugDesiredOpenRef.current = nextOpen;
    if (debugUnmountTimerRef.current !== null) {
      window.clearTimeout(debugUnmountTimerRef.current);
      debugUnmountTimerRef.current = null;
    }
    if (debugOpenFrameRef.current !== null) {
      window.cancelAnimationFrame(debugOpenFrameRef.current);
      debugOpenFrameRef.current = null;
    }
    if (!nextOpen) {
      setDebugOpen(false);
      debugUnmountTimerRef.current = window.setTimeout(() => {
        debugUnmountTimerRef.current = null;
        if (debugDesiredOpenRef.current) return;
        setDebugBodyMounted(false);
      }, DEBUG_CLOSE_ANIMATION_MS);
      return;
    }
    setDebugBodyMounted(true);
    debugOpenFrameRef.current = window.requestAnimationFrame(() => {
      debugOpenFrameRef.current = null;
      if (!debugDesiredOpenRef.current) return;
      setDebugOpen(true);
    });
  }

  function toggleDebug() {
    setDebugVisibility(!debugDesiredOpenRef.current);
  }

  function handleInspectorPointerDown(event: ReactPointerEvent<HTMLElement>) {
    if (!debugDesiredOpenRef.current) return;
    const target = event.target as Node;
    if (debugBodyRef.current?.contains(target)) return;
    if (debugToggleRef.current?.contains(target)) return;
    setDebugVisibility(false);
  }

  useEffect(() => {
    setMetadataOpen(false);
    setMetadataMotion("idle");
  }, [traceStep?.id]);

  useEffect(() => {
    setOutputsOpen(true);
    setSelectedOutputPath(null);
    setOutputPreviewFullscreen(false);
    setOutputActionError(null);
  }, [sessionId]);

  useEffect(() => {
    setSessionCopyState("idle");
    if (sessionCopyTimerRef.current !== null) {
      window.clearTimeout(sessionCopyTimerRef.current);
      sessionCopyTimerRef.current = null;
    }
    return () => {
      if (sessionCopyTimerRef.current !== null) {
        window.clearTimeout(sessionCopyTimerRef.current);
      }
    };
  }, [sessionId]);

  useEffect(() => {
    if (
      selectedOutputPath &&
      !outputArtifacts.some((artifact) => artifact.path === selectedOutputPath)
    ) {
      setSelectedOutputPath(null);
      setOutputPreviewFullscreen(false);
      setOutputActionError(null);
    }
  }, [outputArtifacts, selectedOutputPath]);

  useEffect(() => {
    if (open) return;
    setSelectedOutputPath(null);
    setOutputPreviewFullscreen(false);
    setOutputActionError(null);
  }, [open]);

  useEffect(() => {
    if (!selectedOutputPath) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (outputPreviewFullscreen) setOutputPreviewFullscreen(false);
      else setSelectedOutputPath(null);
      setOutputActionError(null);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [outputPreviewFullscreen, selectedOutputPath]);

  const selectedOutput =
    outputArtifacts.find((artifact) => artifact.path === selectedOutputPath) ?? null;

  useEffect(() => {
    if (!open || !sessionId || outputRequest?.sessionId !== sessionId) return;
    const requestedPath = absoluteArtifactPath(workspaceRoot, outputRequest.path);
    const requestedArtifact = outputArtifacts.find(
      (artifact) => artifact.path === requestedPath
    );
    if (!requestedArtifact) return;
    setOutputsOpen(true);
    selectOutput(requestedArtifact.path);
  }, [open, outputArtifacts, outputRequest, sessionId, workspaceRoot]);

  function selectOutput(path: string) {
    setSelectedOutputPath(path);
    setOutputPreviewFullscreen(false);
    setOutputActionError(null);
  }

  function closeOutputPreview() {
    setSelectedOutputPath(null);
    setOutputPreviewFullscreen(false);
    setOutputActionError(null);
  }

  async function handleOpenOutput() {
    if (!selectedOutput || openingOutputPath) return;
    setOpeningOutputPath(selectedOutput.path);
    setOutputActionError(null);
    try {
      await openArtifact(selectedOutput.path);
    } catch (error) {
      setOutputActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setOpeningOutputPath(null);
    }
  }

  async function handleRevealOutput(path: string) {
    if (openingOutputPath) return;
    setOpeningOutputPath(path);
    setOutputActionError(null);
    try {
      await revealArtifact(path);
    } catch (error) {
      setOutputActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setOpeningOutputPath(null);
    }
  }

  async function handleCopySessionId() {
    if (!sessionId) return;
    if (sessionCopyTimerRef.current !== null) {
      window.clearTimeout(sessionCopyTimerRef.current);
    }
    try {
      await navigator.clipboard.writeText(sessionId);
      setSessionCopyState("copied");
    } catch {
      setSessionCopyState("failed");
    }
    sessionCopyTimerRef.current = window.setTimeout(() => {
      setSessionCopyState("idle");
      sessionCopyTimerRef.current = null;
    }, 1600);
  }

  function beginResize(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    onResizeStart();
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const handleMove = (moveEvent: PointerEvent) => {
      onWidthChange(clampWidth(startWidth + startX - moveEvent.clientX));
    };
    const handleUp = () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      onResizeEnd();
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
  }

  const outputPreview = selectedOutput ? (
    <section
      className="inspector-output-detail"
      aria-label="Output preview"
      aria-modal={outputPreviewFullscreen || undefined}
      data-fullscreen={outputPreviewFullscreen}
      role={outputPreviewFullscreen ? "dialog" : undefined}
    >
      <header title={outputDisplayPath(selectedOutput)}>
        <div className="inspector-output-detail-copy">
          <strong>{artifactName(outputDisplayPath(selectedOutput))}</strong>
          <span>
            {selectedOutput.toolName}
            {selectedOutput.versionCount > 1 ? ` · Version ${selectedOutput.version}` : ""}
          </span>
        </div>
        <div className="inspector-output-actions">
          <button
            className="icon-button quiet"
            type="button"
            aria-label={outputPreviewFullscreen ? "Exit full screen" : "Show full screen"}
            title={outputPreviewFullscreen ? "Exit full screen" : "Show full screen"}
            onClick={() => setOutputPreviewFullscreen((current) => !current)}
          >
            {outputPreviewFullscreen ? (
              <Minimize2 aria-hidden="true" />
            ) : (
              <Maximize2 aria-hidden="true" />
            )}
          </button>
          <button
            className="icon-button quiet"
            type="button"
            aria-label="Open with default app"
            title="Open with default app"
            disabled={openingOutputPath === selectedOutput.path}
            onClick={() => void handleOpenOutput()}
          >
            <ExternalLink aria-hidden="true" />
          </button>
          <button
            className="icon-button quiet"
            type="button"
            aria-label="Close preview"
            title="Close preview"
            onClick={closeOutputPreview}
          >
            <X aria-hidden="true" />
          </button>
        </div>
      </header>
      <div className="inspector-output-action-error" role="alert">
        {outputActionError}
      </div>
      <div className="inspector-output-detail-body">
        <ArtifactPreviewPane path={selectedOutput.path} />
      </div>
    </section>
  ) : null;

  return (
    <aside
      className="inspector"
      aria-label="Inspector"
      data-open={open}
      data-tab={tab}
      onPointerDownCapture={handleInspectorPointerDown}
    >
      <div
        className="inspector-resize-handle"
        role="separator"
        aria-label="Resize inspector"
        aria-orientation="vertical"
        aria-valuemin={280}
        aria-valuemax={520}
        aria-valuenow={width}
        tabIndex={0}
        onPointerDown={beginResize}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft") onWidthChange(clampWidth(width + 16));
          if (event.key === "ArrowRight") onWidthChange(clampWidth(width - 16));
        }}
      />

      <div className="inspector-scroll">
        <section
          className="inspector-outputs"
          aria-label="Agent outputs"
          data-preview-open={Boolean(selectedOutput && outputsOpen)}
        >
          <header>
            <div className="inspector-output-heading">
              <PackageOpen aria-hidden="true" />
              <h2>Outputs</h2>
              {outputArtifacts.length > 0 && (
                <>
                  <span className="inspector-output-count">{outputArtifacts.length}</span>
                  <button
                    className="inspector-output-toggle"
                    type="button"
                    aria-label={outputsOpen ? "Collapse output files" : "Expand output files"}
                    aria-expanded={outputsOpen}
                    title={outputsOpen ? "Collapse output files" : "Expand output files"}
                    onClick={() => setOutputsOpen((current) => !current)}
                  >
                    <ChevronDown
                      className="inspector-output-chevron"
                      data-expanded={outputsOpen}
                      aria-hidden="true"
                    />
                  </button>
                </>
              )}
            </div>
          </header>
          {outputArtifacts.length === 0 ? (
            <div className="inspector-output-empty">
              <span>Files and images created by the agent appear here.</span>
            </div>
          ) : !outputsOpen ? null : selectedOutput ? (
            outputPreviewFullscreen ? null : outputPreview
          ) : (
              <div className="inspector-output-list">
                {outputArtifacts.map((artifact) => {
                  const displayPath = outputDisplayPath(artifact);
                  const versionLabel =
                    artifact.versionCount > 1 ? `Version ${artifact.version}` : null;
                  return (
                    <div
                      className="inspector-output-row"
                      key={`${artifact.id}-${artifact.path}`}
                    >
                      <button
                        className="inspector-output"
                        type="button"
                        aria-label={`Preview ${artifactName(displayPath)}${
                          versionLabel ? `, ${versionLabel}` : ""
                        }`}
                        title={`Preview ${artifactName(displayPath)}${
                          versionLabel ? `, ${versionLabel}` : ""
                        }`}
                        onClick={() => selectOutput(artifact.path)}
                      >
                        <ArtifactTypeIcon path={artifact.path} />
                        <div className="inspector-output-name">
                          <strong title={displayPath}>{artifactName(displayPath)}</strong>
                          {versionLabel && <small>v{artifact.version}</small>}
                        </div>
                      </button>
                      <button
                        className="inspector-output-reveal"
                        type="button"
                        aria-label={`Show ${artifactName(displayPath)} in Finder`}
                        title="Show in Finder"
                        disabled={openingOutputPath === artifact.path}
                        onClick={() => void handleRevealOutput(artifact.path)}
                      >
                        <FolderOpen aria-hidden="true" />
                      </button>
                    </div>
                  );
                })}
              </div>
          )}
          {outputsOpen && !selectedOutput && outputActionError && (
            <div className="inspector-output-action-error" role="alert">
              {outputActionError}
            </div>
          )}
        </section>
      </div>

      <section className="inspector-debug" data-open={debugOpen} hidden={!showDebug}>
        {debugBodyMounted && (
        <div
          ref={debugBodyRef}
          className="inspector-debug-body"
          id="inspector-debug-panel"
          aria-hidden={!debugOpen}
        >
          <div className="inspector-debug-session">
            <span>Session ID</span>
            <code title={sessionId ?? "No active session"}>
              {sessionId ?? "No active session"}
            </code>
            <button
              className="icon-button quiet"
              type="button"
              disabled={!sessionId}
              aria-label="Copy session ID"
              title={sessionCopyState === "copied" ? "Copied" : "Copy session ID"}
              onClick={() => void handleCopySessionId()}
            >
              {sessionCopyState === "copied" ? (
                <Check aria-hidden="true" />
              ) : (
                <Copy aria-hidden="true" />
              )}
            </button>
          </div>
          <nav className="inspector-tabs" aria-label="Debug views" role="tablist">
              {(["trace", "details", "artifacts", "context"] as InspectorTab[]).map((item, index, tabs) => (
                <button
                  className={`inspector-tab ${tab === item ? "active" : ""}`}
                  type="button"
                  role="tab"
                  aria-selected={tab === item}
                  aria-controls="inspector-panel"
                  key={item}
                  onClick={() => onTabChange(item)}
                  onKeyDown={(event) => {
                    const delta = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
                    if (!delta) return;
                    event.preventDefault();
                    const nextIndex = (index + delta + tabs.length) % tabs.length;
                    onTabChange(tabs[nextIndex]);
                    const buttons = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
                      '[role="tab"]'
                    );
                    buttons?.[nextIndex]?.focus();
                  }}
                >
                  {item === "trace"
                    ? "Trace"
                    : item === "details"
                      ? "Details"
                      : item === "artifacts"
                        ? "Artifacts"
                        : "Context"}
                </button>
              ))}
          </nav>

        {tab === "trace" && (
          <div className="inspector-panel inspector-trace-panel" id="inspector-panel" role="tabpanel">
            <section className="inspector-trace-summary">
              <dl className="detail-list">
                <div>
                  <dt>Status</dt>
                  <dd><TraceStatusIcon status={agentStatus} /></dd>
                </div>
                <div>
                  <dt>Turns</dt>
                  <dd>{traceSummary.turnCount}</dd>
                </div>
                <div>
                  <dt>Steps</dt>
                  <dd>{sessionTraceSteps.length}</dd>
                </div>
                <div>
                  <dt>Tools</dt>
                  <dd>{traceSummary.toolCallCount}</dd>
                </div>
                <div>
                  <dt>Permissions</dt>
                  <dd>{traceSummary.permissionCount}</dd>
                </div>
              </dl>
              {roleSummaries.length > 0 && (
                <dl className="detail-list inspector-role-summary" aria-label="Model role activity">
                  {roleSummaries.map((summary) => (
                    <div key={summary.role}>
                      <dt>{summary.role}</dt>
                      <dd>
                        <span>
                          {summary.calls} calls · {formatDuration(summary.latencyMs)}
                          {summary.firstTokenLatencyMs != null
                            ? ` · TTFT ${formatDuration(summary.firstTokenLatencyMs)}`
                            : ""}
                          {` · ${summary.evidenceCount} evidence`}
                        </span>
                        <small>{summary.models.join(", ") || "unreported model"}</small>
                      </dd>
                    </div>
                  ))}
                </dl>
              )}
              <button
                className="secondary-button inspector-trace-export"
                type="button"
                disabled={traceBusy || !sessionId}
                onClick={onTraceExport}
              >
                <Save aria-hidden="true" />
                <span>{traceBusy ? "Exporting" : "Export JSONL"}</span>
              </button>
              {traceExportPath && <p className="inspector-path">{traceExportPath}</p>}
            </section>
            <section className="inspector-trace-sequence" aria-label="Agent trace steps">
              {sessionTraceSteps.length === 0 ? (
                <div className="inspector-empty">
                  <Clock3 aria-hidden="true" />
                  <span>No agent trace yet</span>
                </div>
              ) : (
                <ol className="inspector-trace-list">
                  {sessionTraceSteps.map((step, index) => (
                    <li className="trace-sequence-item" key={step.id}>
                      <span className="trace-sequence-marker" aria-hidden="true">
                        {index + 1}
                      </span>
                      <button
                        className={`trace-step ${traceStep?.id === step.id ? "selected" : ""}`}
                        type="button"
                        aria-label={`Step ${index + 1}: ${step.label}`}
                        onClick={() => onTraceStepSelect(step.id)}
                      >
                        <span className="trace-step-icon"><TraceIcon step={step} /></span>
                        <span className="trace-step-body">
                          <strong>{step.label}</strong>
                          <small>{step.detail}</small>
                        </span>
                        <span className="trace-step-meta">
                          <TraceStatusIcon status={step.status} />
                          <small>
                            {step.turnIndex > 0 ? `Turn ${step.turnIndex}` : "Setup"} · {formatDuration(step.latencyMs)}
                          </small>
                        </span>
                      </button>
                    </li>
                  ))}
                </ol>
              )}
            </section>
          </div>
        )}

        {tab === "details" && (
          <div className="inspector-panel" id="inspector-panel" role="tabpanel">
            {traceStep ? (
              <section className="inspector-section">
                <div className="section-title">
                  <Activity aria-hidden="true" />
                  <h2>{traceStep.label}</h2>
                </div>
                <dl className="detail-list">
                  <div>
                    <dt>Status</dt>
                    <dd>
                      <TraceStatusIcon status={traceStep.status} />
                    </dd>
                  </div>
                  <div>
                    <dt>Duration</dt>
                    <dd>{formatDuration(traceStep.latencyMs)}</dd>
                  </div>
                  <div>
                    <dt>Turn</dt>
                    <dd>{traceStep.turnIndex}</dd>
                  </div>
                  <div>
                    <dt>Model</dt>
                    <dd>{traceStep.model ?? "none"}</dd>
                  </div>
                  <div>
                    <dt>Tool</dt>
                    <dd>{traceStep.toolName ?? "none"}</dd>
                  </div>
                  <div>
                    <dt>Parent</dt>
                    <dd>{traceStep.parentId ?? "root"}</dd>
                  </div>
                </dl>
                <pre className="inspector-code">{traceStep.detail}</pre>
                {(traceStep.inputPreview || traceStep.outputPreview) && (
                  <pre className="inspector-code">
                    {[
                      traceStep.inputPreview ? `input:\n${traceStep.inputPreview}` : "",
                      traceStep.outputPreview ? `output:\n${traceStep.outputPreview}` : ""
                    ]
                      .filter(Boolean)
                      .join("\n\n")}
                  </pre>
                )}
                <div
                  className="metadata-details"
                  data-open={metadataOpen}
                  data-motion={metadataMotion}
                >
                  <button
                    type="button"
                    className="metadata-details-toggle"
                    aria-expanded={metadataOpen}
                    title={metadataOpen ? "Hide metadata" : "Show metadata"}
                    onClick={() => {
                      const nextOpen = !metadataOpen;
                      setMetadataMotion(nextOpen ? "opening" : "closing");
                      setMetadataOpen(nextOpen);
                    }}
                  >
                    <span>Metadata</span>
                    <span
                      className="metadata-disclosure-icon"
                      onAnimationEnd={(event) => {
                        if (event.animationName.startsWith("metadata-disclosure-")) {
                          setMetadataMotion("idle");
                        }
                      }}
                    >
                      <ChevronRight
                        className="metadata-disclosure-chevron"
                        aria-hidden="true"
                      />
                    </span>
                  </button>
                  <div className="metadata-details-body" aria-hidden={!metadataOpen}>
                    <div>
                      <pre className="inspector-code">
                        {JSON.stringify(traceStep.metadata, null, 2)}
                      </pre>
                    </div>
                  </div>
                </div>
                {traceExportPath && <p className="inspector-path">{traceExportPath}</p>}
              </section>
            ) : threadSelection ? (
              <section className="inspector-section">
                <div className="section-title">
                  {threadSelection.type === "event" ? (
                    <Activity aria-hidden="true" />
                  ) : (
                    <FileText aria-hidden="true" />
                  )}
                  <h2>
                    {threadSelection.type === "event"
                      ? threadSelection.event.label
                      : threadSelection.message.role}
                  </h2>
                </div>
                <dl className="detail-list">
                  <div>
                    <dt>Time</dt>
                    <dd>
                      {formatTime(
                        threadSelection.type === "event"
                          ? threadSelection.event.timestampMs
                          : threadSelection.message.timestampMs
                      )}
                    </dd>
                  </div>
                  <div>
                    <dt>Type</dt>
                    <dd>
                      {threadSelection.type === "event"
                        ? threadSelection.event.kind
                        : threadSelection.message.role}
                    </dd>
                  </div>
                </dl>
                <pre className="inspector-code">
                  {threadSelection.type === "event"
                    ? threadSelection.event.detail
                    : threadSelection.message.content}
                </pre>
              </section>
            ) : (
              <section className="inspector-section">
                <div className="section-title">
                  <Activity aria-hidden="true" />
                  <h2>Run</h2>
                </div>
                <dl className="detail-list">
                  <div>
                    <dt>Status</dt>
                    <dd>{agentStatus}</dd>
                  </div>
                  <div>
                    <dt>Turns</dt>
                    <dd>
                      {agentTurnCount}/{agentMaxTurns}
                    </dd>
                  </div>
                </dl>
                <div className="inspector-empty">
                  <Clock3 aria-hidden="true" />
                  <span>Select an event or trace step</span>
                </div>
              </section>
            )}

            {reviewTotal > 0 && (
              <section className="inspector-section inspector-review">
                <div className="section-title">
                  <ShieldCheck aria-hidden="true" />
                  <h2>Review required</h2>
                </div>
                <p>{reviewTotal} pending permission request{reviewTotal === 1 ? "" : "s"}</p>
                <button className="secondary-button" type="button" onClick={onReview}>
                  Review
                </button>
              </section>
            )}
          </div>
        )}

        {tab === "artifacts" && (
          <div className="inspector-panel" id="inspector-panel" role="tabpanel">
            {!hasArtifacts && <div className="inspector-empty">No artifacts yet</div>}

            {(ragAnswer || ragSources.length > 0) && (
              <section className="inspector-section">
                <div className="section-title">
                  <Database aria-hidden="true" />
                  <h2>Knowledge</h2>
                </div>
                {ragAnswer && <pre className="inspector-code">{ragAnswer}</pre>}
                {ragSources.slice(0, 4).map((source) => (
                  <article className="artifact-row" key={`${source.path}-${source.startLine}-${source.fileHash}`}>
                    <header>
                      <strong>{source.path}</strong>
                      <span>{source.score.toFixed(3)}</span>
                    </header>
                    <p>
                      Lines {source.startLine}-{source.endLine} · {source.reason}
                    </p>
                    <pre className="inspector-code">{source.text}</pre>
                  </article>
                ))}
              </section>
            )}

            {browserObservations.length > 0 && (
              <section className="inspector-section">
                <div className="section-title">
                  <Globe2 aria-hidden="true" />
                  <h2>Browser</h2>
                </div>
                {browserObservations.slice(-4).map((observation) => (
                  <article className="artifact-row" key={observation.invocationId}>
                    <header>
                      <strong>{observation.toolName}</strong>
                      <span>{observation.status}</span>
                    </header>
                    {observation.url && <p>{observation.url}</p>}
                    <pre className="inspector-code">{observation.output || "No output"}</pre>
                    {(observation.artifactPath || observation.textPath) && (
                      <p className="inspector-path">
                        {observation.artifactPath ?? observation.textPath}
                      </p>
                    )}
                  </article>
                ))}
              </section>
            )}

            {toolResults.length > 0 && (
              <section className="inspector-section">
                <div className="section-title">
                  <TerminalSquare aria-hidden="true" />
                  <h2>Tools</h2>
                </div>
                {toolResults.slice(-4).map((result) => (
                  <article className="artifact-row" key={result.invocationId}>
                    <header>
                      <strong>{result.toolName}</strong>
                      <span>{result.status}</span>
                    </header>
                    <pre className="inspector-code">{result.output || "No output"}</pre>
                  </article>
                ))}
              </section>
            )}
          </div>
        )}

        {tab === "context" && (
          <div className="inspector-panel" id="inspector-panel" role="tabpanel">
            {!hasContext || !contextCheckpoint ? (
              <div className="inspector-empty">No context checkpoint yet</div>
            ) : (
              <section className="inspector-section">
                <div className="section-title">
                  <FileText aria-hidden="true" />
                  <h2>Context checkpoint</h2>
                </div>
                <dl className="detail-list">
                  <div>
                    <dt>Events</dt>
                    <dd>{contextCheckpoint.eventCount}</dd>
                  </div>
                  <div>
                    <dt>Tasks</dt>
                    <dd>{contextCheckpoint.taskCount}</dd>
                  </div>
                  <div>
                    <dt>Generated</dt>
                    <dd>{formatTime(contextCheckpoint.generatedAtMs)}</dd>
                  </div>
                </dl>
                <pre className="inspector-code">{contextCheckpoint.restorePack}</pre>
                {contextCheckpoint.path && (
                  <p className="inspector-path">{contextCheckpoint.path}</p>
                )}
              </section>
            )}
          </div>
        )}
        </div>
        )}
        <button
          ref={debugToggleRef}
          className="inspector-debug-toggle"
          type="button"
          aria-label={debugOpen ? "Hide debug and trace" : "Show debug and trace"}
          aria-expanded={debugOpen}
          aria-controls="inspector-debug-panel"
          title={debugOpen ? "Hide debug and trace" : "Show debug and trace"}
          onClick={toggleDebug}
        >
          <Bug aria-hidden="true" />
          <strong>Debug</strong>
          <span
            className="inspector-debug-chevron"
            data-state={debugOpen ? "expanded" : "collapsed"}
            aria-hidden="true"
          >
            <ChevronDown />
          </span>
        </button>
      </section>
      {outputPreviewFullscreen && outputPreview
        ? createPortal(outputPreview, document.body)
        : null}
      {sessionCopyState !== "idle"
        ? createPortal(
            <div
              className="clipboard-toast"
              data-failed={sessionCopyState === "failed"}
              role="status"
            >
              {sessionCopyState === "failed" ? (
                <TriangleAlert aria-hidden="true" />
              ) : (
                <CheckCircle2 aria-hidden="true" />
              )}
              <span>
                {sessionCopyState === "failed"
                  ? "Could not copy session ID"
                  : "Session ID copied"}
              </span>
            </div>,
            document.body
          )
        : null}
    </aside>
  );
}
