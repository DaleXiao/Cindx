import {
  CheckCircle2,
  CircleAlert,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen
} from "lucide-react";
import type { WorkspaceView } from "./Sidebar";

type WorkspaceChromeProps = {
  activeView: WorkspaceView;
  contextEstimated: boolean;
  contextRemainingPercent: number;
  contextTokensUsed: number;
  contextWindowTokens: number;
  inspectorOpen: boolean;
  onInspectorToggle: () => void;
  onSidebarToggle: () => void;
  sidebarOpen: boolean;
  statusText: string;
  title: string;
};

function formatTokenCount(tokens: number) {
  if (tokens < 1000) return String(tokens);
  if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(tokens < 10_000 ? 1 : 0)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

export function WorkspaceChrome({
  activeView,
  contextEstimated,
  contextRemainingPercent,
  contextTokensUsed,
  contextWindowTokens,
  inspectorOpen,
  onInspectorToggle,
  onSidebarToggle,
  sidebarOpen,
  statusText,
  title
}: WorkspaceChromeProps) {
  return (
    <>
      <header className="window-toolbar" data-tauri-drag-region>
        <span className="window-toolbar-panel window-toolbar-panel-left" aria-hidden="true" />
        <span className="window-toolbar-panel window-toolbar-panel-right" aria-hidden="true" />
        {activeView !== "settings" && (
          <button
            className="window-pane-toggle sidebar-pane-toggle"
            type="button"
            aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
            aria-pressed={sidebarOpen}
            data-open={sidebarOpen}
            title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
            onClick={onSidebarToggle}
          >
            <span className="window-pane-toggle-icon" aria-hidden="true">
              <PanelLeftClose className="pane-icon-open" />
              <PanelLeftOpen className="pane-icon-closed" />
            </span>
          </button>
        )}
        {activeView === "timeline" && (
          <button
            className="window-pane-toggle inspector-pane-toggle"
            type="button"
            aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}
            aria-pressed={inspectorOpen}
            data-open={inspectorOpen}
            title={inspectorOpen ? "Hide inspector" : "Show inspector"}
            onClick={onInspectorToggle}
          >
            <span className="window-pane-toggle-icon" aria-hidden="true">
              <PanelRightClose className="pane-icon-open" />
              <PanelRightOpen className="pane-icon-closed" />
            </span>
          </button>
        )}
      </header>

      <div className="window-workspace-header">
        <div className="topbar-title">
          <div>
            <h1>{title}</h1>
          </div>
        </div>
        {activeView === "timeline" && (
          <div className="topbar-actions">
            <div
              className="context-usage"
              title={`${contextTokensUsed} of ${contextWindowTokens} context tokens${contextEstimated ? " (estimated)" : ""}`}
            >
              <span>
                {contextEstimated ? "~" : ""}
                {formatTokenCount(contextTokensUsed)} tokens
              </span>
              <strong>{Math.round(contextRemainingPercent)}% left</strong>
              <progress
                max={100}
                value={contextRemainingPercent}
                aria-label="Context window remaining"
              />
            </div>
            <div className={`runtime-pill ${statusText === "Ready" ? "ready" : ""}`}>
              {statusText === "Ready" ? (
                <CheckCircle2 size={16} aria-hidden="true" />
              ) : (
                <CircleAlert size={16} aria-hidden="true" />
              )}
              <span>{statusText}</span>
            </div>
          </div>
        )}
      </div>
    </>
  );
}
