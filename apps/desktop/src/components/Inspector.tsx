import {
  Activity,
  Bug,
  Clock3,
  Database,
  ExternalLink,
  File,
  FileText,
  Globe2,
  Image,
  Maximize2,
  Minimize2,
  ShieldCheck,
  TerminalSquare,
  X
} from "lucide-react";
import Markdown from "markdown-to-jsx";
import { createPortal } from "react-dom";
import {
  useEffect,
  useMemo,
  useState,
  type PointerEvent as ReactPointerEvent
} from "react";
import { openArtifact, readArtifactPreview } from "../tauri";
import type {
  AgentState,
  AgentTraceStepView,
  ArtifactPreview,
  BrowserObservationView,
  ContextCheckpointView,
  RagSourceView,
  ToolRunView
} from "../tauri";
import type { SessionThreadSelection } from "./SessionThread";
import { DisclosureTriangle } from "./DisclosureTriangle";
import { TraceStatusIcon } from "./TraceStatusIcon";

export type InspectorTab = "details" | "artifacts" | "context";

type ReviewCounts = {
  agent: number;
  tool: number;
  browser: number;
};

type InspectorProps = {
  open: boolean;
  width: number;
  tab: InspectorTab;
  sessionId: string | null;
  threadSelection: SessionThreadSelection | null;
  traceStep: AgentTraceStepView | null;
  traceExportPath: string | null;
  contextCheckpoint: ContextCheckpointView | null;
  ragAnswer: string | null;
  ragSources: RagSourceView[];
  browserObservations: BrowserObservationView[];
  toolResults: ToolRunView[];
  sessionTraceSteps: AgentTraceStepView[];
  workspaceRoot: string;
  agentStatus: AgentState["status"] | "idle";
  agentTurnCount: number;
  agentMaxTurns: number;
  reviewCounts: ReviewCounts;
  onTabChange: (tab: InspectorTab) => void;
  onWidthChange: (width: number) => void;
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

type OutputArtifact = {
  id: string;
  path: string;
  toolName: string;
  status: string;
  timestampMs: number;
};

const IMAGE_EXTENSIONS = new Set(["avif", "bmp", "gif", "jpeg", "jpg", "png", "webp"]);
const MARKDOWN_EXTENSIONS = new Set(["md", "mdown", "markdown"]);
const HTML_EXTENSIONS = new Set(["htm", "html"]);

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

function sessionArtifactPaths(step: AgentTraceStepView) {
  const paths = new Set<string>();
  if (step.artifactPath) paths.add(step.artifactPath);

  Object.entries(step.metadata).forEach(([key, path]) => {
    const resultPath = key.startsWith("result_") && key.endsWith("_path");
    const fileReference =
      key === "result_path" && (step.toolName === "file.read" || step.toolName === "file.write");
    const contextPath = key === "context_checkpoint_path" || key === "lancedb_export_path";
    if ((resultPath && (key !== "result_path" || fileReference)) || contextPath) {
      if (path.trim()) paths.add(path);
    }
  });

  return [...paths];
}

function ArtifactTypeIcon({ path }: { path: string }) {
  const extension = artifactExtension(path);
  if (IMAGE_EXTENSIONS.has(extension)) return <Image aria-hidden="true" />;
  if (MARKDOWN_EXTENSIONS.has(extension)) return <FileText aria-hidden="true" />;
  if (HTML_EXTENSIONS.has(extension)) return <Globe2 aria-hidden="true" />;
  return <File aria-hidden="true" />;
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
  width,
  tab,
  sessionId,
  threadSelection,
  traceStep,
  traceExportPath,
  contextCheckpoint,
  ragAnswer,
  ragSources,
  browserObservations,
  toolResults,
  sessionTraceSteps,
  workspaceRoot,
  agentStatus,
  agentTurnCount,
  agentMaxTurns,
  reviewCounts,
  onTabChange,
  onWidthChange,
  onReview
}: InspectorProps) {
  const [debugOpen, setDebugOpen] = useState(false);
  const [selectedOutputPath, setSelectedOutputPath] = useState<string | null>(null);
  const [outputPreviewFullscreen, setOutputPreviewFullscreen] = useState(false);
  const [openingOutputPath, setOpeningOutputPath] = useState<string | null>(null);
  const [outputActionError, setOutputActionError] = useState<string | null>(null);
  const reviewTotal = reviewCounts.agent + reviewCounts.tool + reviewCounts.browser;
  const hasArtifacts = Boolean(ragAnswer || ragSources.length || browserObservations.length || toolResults.length);
  const hasContext = Boolean(
    contextCheckpoint && (contextCheckpoint.eventCount > 0 || contextCheckpoint.path)
  );
  const outputArtifacts = useMemo(() => {
    const outputs = new Map<string, OutputArtifact>();
    const addOutput = (artifact: OutputArtifact) => {
      const absolutePath = absoluteArtifactPath(workspaceRoot, artifact.path);
      const current = outputs.get(absolutePath);
      if (!current || artifact.timestampMs >= current.timestampMs) {
        outputs.set(absolutePath, { ...artifact, path: absolutePath });
      }
    };

    sessionTraceSteps.forEach((step) => {
      if (step.status === "failed") return;
      sessionArtifactPaths(step).forEach((path, index) => {
        addOutput({
          id: `${step.id}-${index}`,
          path,
          toolName: step.toolName ?? step.label,
          status: step.toolName === "file.read" ? "reference" : step.status,
          timestampMs: step.finishedAtMs ?? step.startedAtMs
        });
      });
    });

    return [...outputs.values()].sort((left, right) => right.timestampMs - left.timestampMs);
  }, [sessionTraceSteps, workspaceRoot]);

  useEffect(() => {
    setSelectedOutputPath(null);
    setOutputPreviewFullscreen(false);
    setOutputActionError(null);
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

  function beginResize(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;

    const handleMove = (moveEvent: PointerEvent) => {
      onWidthChange(clampWidth(startWidth + startX - moveEvent.clientX));
    };
    const handleUp = () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
  }

  const outputPreview = selectedOutput ? (
    <section
      className="inspector-output-detail"
      aria-label="Output preview"
      aria-modal={outputPreviewFullscreen || undefined}
      data-fullscreen={outputPreviewFullscreen}
      role={outputPreviewFullscreen ? "dialog" : undefined}
    >
      <header title={selectedOutput.path}>
        <div className="inspector-output-detail-copy">
          <strong>{artifactName(selectedOutput.path)}</strong>
          <span>{selectedOutput.toolName}</span>
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
          data-preview-open={Boolean(selectedOutput)}
        >
          <header>
            <div>
              <File aria-hidden="true" />
              <h2>Outputs</h2>
            </div>
            {outputArtifacts.length > 0 && <span>{outputArtifacts.length}</span>}
          </header>
          {outputArtifacts.length === 0 ? (
            <div className="inspector-output-empty">
              <span>Files and images created by the agent appear here.</span>
            </div>
          ) : selectedOutput ? (
            outputPreviewFullscreen ? null : outputPreview
          ) : (
              <div className="inspector-output-list">
                {outputArtifacts.map((artifact) => {
                  return (
                    <button
                      className="inspector-output"
                      type="button"
                      aria-label={`Preview ${artifactName(artifact.path)}`}
                      key={`${artifact.id}-${artifact.path}`}
                      onClick={() => selectOutput(artifact.path)}
                    >
                      <div className="inspector-output-preview">
                        <ArtifactTypeIcon path={artifact.path} />
                      </div>
                      <div className="inspector-output-copy">
                        <strong title={artifact.path}>{artifactName(artifact.path)}</strong>
                        <span title={artifact.path}>{artifact.path}</span>
                      </div>
                    </button>
                  );
                })}
              </div>
          )}
        </section>
      </div>

      <section className="inspector-debug" data-open={debugOpen}>
        <div
          className="inspector-debug-body"
          id="inspector-debug-panel"
          aria-hidden={!debugOpen}
        >
          <nav className="inspector-tabs" aria-label="Debug views" role="tablist">
              {(["details", "artifacts", "context"] as InspectorTab[]).map((item, index, tabs) => (
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
                  {item === "details" ? "Details" : item === "artifacts" ? "Artifacts" : "Context"}
                </button>
              ))}
          </nav>

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
                <details className="metadata-details">
                  <summary>
                    <DisclosureTriangle />
                    <span>Metadata</span>
                  </summary>
                  <pre className="inspector-code">{JSON.stringify(traceStep.metadata, null, 2)}</pre>
                </details>
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
        <button
          className="inspector-debug-toggle"
          type="button"
          aria-expanded={debugOpen}
          aria-controls="inspector-debug-panel"
          onClick={() => setDebugOpen((current) => !current)}
        >
          <Bug aria-hidden="true" />
          <strong>Debug</strong>
          <DisclosureTriangle />
        </button>
      </section>
      {outputPreviewFullscreen && outputPreview
        ? createPortal(outputPreview, document.body)
        : null}
    </aside>
  );
}
