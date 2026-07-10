import {
  Activity,
  Clock3,
  Database,
  FileText,
  Globe2,
  ShieldCheck,
  TerminalSquare
} from "lucide-react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type {
  AgentState,
  AgentTraceStepView,
  BrowserObservationView,
  ContextCheckpointView,
  RagSourceView,
  ToolRunView
} from "../tauri";
import type { SessionThreadSelection } from "./SessionThread";

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
  threadSelection: SessionThreadSelection | null;
  traceStep: AgentTraceStepView | null;
  traceExportPath: string | null;
  contextCheckpoint: ContextCheckpointView | null;
  ragAnswer: string | null;
  ragSources: RagSourceView[];
  browserObservations: BrowserObservationView[];
  toolResults: ToolRunView[];
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

export function Inspector({
  open,
  width,
  tab,
  threadSelection,
  traceStep,
  traceExportPath,
  contextCheckpoint,
  ragAnswer,
  ragSources,
  browserObservations,
  toolResults,
  agentStatus,
  agentTurnCount,
  agentMaxTurns,
  reviewCounts,
  onTabChange,
  onWidthChange,
  onReview
}: InspectorProps) {
  const reviewTotal = reviewCounts.agent + reviewCounts.tool + reviewCounts.browser;
  const hasArtifacts = Boolean(ragAnswer || ragSources.length || browserObservations.length || toolResults.length);
  const hasContext = Boolean(
    contextCheckpoint && (contextCheckpoint.eventCount > 0 || contextCheckpoint.path)
  );

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

      <nav className="inspector-tabs" aria-label="Inspector views" role="tablist">
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

      <div className="inspector-scroll">
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
                    <dd>{traceStep.status}</dd>
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
                  <summary>Metadata</summary>
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
    </aside>
  );
}
