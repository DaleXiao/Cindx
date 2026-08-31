import { useCallback, useEffect, useRef, useState } from "react";
import { FileDiff, FilePlus2, History, Redo2, Undo2 } from "lucide-react";
import type { WorkspaceUndoEntryView, WorkspaceUndoState } from "../tauriTypes";
import {
  getWorkspaceUndoState,
  redoWorkspaceChange,
  undoWorkspaceChange
} from "../tauri";

type FileChangesPanelProps = {
  sessionId: string | null;
  /** Agent run status; a transition (e.g. running → completed) refetches the list. */
  status: string;
  working?: boolean;
};

function actionIcon(entry: WorkspaceUndoEntryView) {
  return entry.action === "created" ? (
    <FilePlus2 size={13} aria-hidden="true" />
  ) : (
    <FileDiff size={13} aria-hidden="true" />
  );
}

/**
 * Session file-change history shown above the Composer once the agent has
 * touched workspace files: the File changes summary and Undo/Redo actions sit
 * on the header row, and every changed file is listed below (newest first)
 * with its action, tool, and undone state.
 */
export function FileChangesPanel({ sessionId, status, working }: FileChangesPanelProps) {
  const [state, setState] = useState<WorkspaceUndoState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const activeSessionRef = useRef(sessionId);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      setState(null);
      return;
    }
    try {
      const next = await getWorkspaceUndoState(sessionId);
      activeSessionRef.current = sessionId;
      setState(next);
      setError(null);
    } catch (refreshError) {
      setError(String(refreshError));
    }
  }, [sessionId]);

  useEffect(() => {
    activeSessionRef.current = sessionId;
    void refresh();
  }, [sessionId, status, refresh]);

  if (!sessionId || !state || state.entries.length === 0) {
    return null;
  }

  const applyChange = async (action: "undo" | "redo") => {
    if (busy || working || !sessionId) return;
    setBusy(true);
    setError(null);
    try {
      const next =
        action === "undo"
          ? await undoWorkspaceChange(sessionId)
          : await redoWorkspaceChange(sessionId);
      if (activeSessionRef.current === sessionId) {
        setState(next);
      }
    } catch (actionError) {
      setError(String(actionError));
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const latestEntry = [...state.entries].reverse().find((entry) => !entry.undone);
  const undoneEntries = state.entries.filter((entry) => entry.undone);
  const redoEntry = undoneEntries[undoneEntries.length - 1];
  const changeCount = state.entries.length;
  const rows = [...state.entries].reverse();

  return (
    <section className="file-changes-panel" role="group" aria-label="Agent file change history">
      <div className="file-changes-header">
        <History size={13} aria-hidden="true" />
        <strong>File changes ({changeCount})</strong>
        {error ? (
          <span className="file-changes-error" role="alert">
            {error}
          </span>
        ) : null}
        <span className="file-changes-actions">
          <button
            type="button"
            className="file-changes-button"
            disabled={busy || working || !state.canUndo}
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
            disabled={busy || working || !state.canRedo}
            onClick={() => void applyChange("redo")}
            title={
              redoEntry
                ? `Restore the undone change to ${redoEntry.path}`
                : "Restore the last undone file change"
            }
          >
            <Redo2 size={13} aria-hidden="true" />
            <span>Redo</span>
          </button>
        </span>
      </div>
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
    </section>
  );
}
