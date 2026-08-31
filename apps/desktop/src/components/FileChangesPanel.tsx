import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp, FileDiff, FilePlus2, History, Redo2, Undo2 } from "lucide-react";
import type { WorkspaceUndoEntryView } from "../tauriTypes";
import { redoWorkspaceChange, undoWorkspaceChange } from "../tauri";

type FileChangesPanelProps = {
  sessionId: string | null;
  /** The file changes produced by this panel's run, in projection order. */
  entries: WorkspaceUndoEntryView[];
  canUndo: boolean;
  canRedo: boolean;
  /** The most recent finished run: expanded by default and owns Undo/Redo. */
  isLatest: boolean;
  working?: boolean;
  onStateChanged: () => void;
};

function actionIcon(entry: WorkspaceUndoEntryView) {
  return entry.action === "created" ? (
    <FilePlus2 size={13} aria-hidden="true" />
  ) : (
    <FileDiff size={13} aria-hidden="true" />
  );
}

/**
 * One run's file-change list, rendered in the thread right after the turn
 * that produced it. The latest run's panel is expanded and carries the
 * session-level Undo/Redo actions (undo is a session LIFO, so older panels
 * stay list-only); when the next turn starts the panel demotes and collapses,
 * remaining reviewable in the history.
 */
export function FileChangesPanel({
  sessionId,
  entries,
  canUndo,
  canRedo,
  isLatest,
  working,
  onStateChanged
}: FileChangesPanelProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(isLatest);

  // A new turn starting demotes this panel: collapse it automatically.
  useEffect(() => {
    if (!isLatest) setExpanded(false);
    else setExpanded(true);
  }, [isLatest]);

  if (entries.length === 0) return null;

  const applyChange = async (action: "undo" | "redo") => {
    if (busy || working || !sessionId) return;
    setBusy(true);
    setError(null);
    try {
      await (action === "undo"
        ? undoWorkspaceChange(sessionId)
        : redoWorkspaceChange(sessionId));
      onStateChanged();
    } catch (actionError) {
      setError(String(actionError));
    } finally {
      setBusy(false);
    }
  };

  const latestEntry = [...entries].reverse().find((entry) => !entry.undone);
  const rows = [...entries].reverse();
  const changeCount = entries.length;

  return (
    <section
      className="file-changes-panel"
      role="group"
      aria-label="Agent file change history"
      data-expanded={expanded || undefined}
    >
      <div className="file-changes-header">
        <button
          type="button"
          className="file-changes-toggle"
          onClick={() => setExpanded((current) => !current)}
          aria-expanded={expanded}
          title={expanded ? "Collapse this run's file changes" : "Expand this run's file changes"}
        >
          <History size={13} aria-hidden="true" />
          <strong>File changes ({changeCount})</strong>
          {expanded ? <ChevronUp size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
        </button>
        {error ? (
          <span className="file-changes-error" role="alert">
            {error}
          </span>
        ) : null}
        {isLatest && (
          <span className="file-changes-actions">
            <button
              type="button"
              className="file-changes-button"
              disabled={busy || working || !canUndo}
              onClick={() => void applyChange("undo")}
              title={
                latestEntry
                  ? `Undo the agent's change to ${latestEntry.path}`
                  : "Undo the agent's last file change"
              }
            >
              <Undo2 size={13} aria-hidden="true" />
              <span>Undo</span>
            </button>
            <button
              type="button"
              className="file-changes-button"
              disabled={busy || working || !canRedo}
              onClick={() => void applyChange("redo")}
              title="Restore the last undone file change"
            >
              <Redo2 size={13} aria-hidden="true" />
              <span>Redo</span>
            </button>
          </span>
        )}
      </div>
      {expanded && (
        <ol className="file-changes-list">
          {rows.map((entry) => (
            <li
              key={`${entry.toolCallId}-${entry.sequence}`}
              className="file-changes-row"
              data-undone={entry.undone || undefined}
            >
              {actionIcon(entry)}
              <code className="file-changes-path" title={entry.path}>
                {entry.path}
              </code>
              <span className="file-changes-action">{entry.action}</span>
              <span className="file-changes-tool">{entry.tool}</span>
              {entry.undone && <span className="file-changes-undone">undone</span>}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
