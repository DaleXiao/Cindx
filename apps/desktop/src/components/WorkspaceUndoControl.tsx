import { useCallback, useEffect, useRef, useState } from "react";
import type { WorkspaceUndoState } from "../tauriTypes";
import {
  getWorkspaceUndoState,
  redoWorkspaceChange,
  undoWorkspaceChange
} from "../tauri";

type WorkspaceUndoControlProps = {
  sessionId: string | null;
  disabled?: boolean;
};

export function WorkspaceUndoControl({ sessionId, disabled }: WorkspaceUndoControlProps) {
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
  }, [sessionId, refresh]);

  if (!sessionId || !state || state.entries.length === 0) {
    return null;
  }

  const applyChange = async (action: "undo" | "redo") => {
    if (busy || disabled || !sessionId) return;
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

  return (
    <div className="composer-undo-control" role="group" aria-label="Workspace change history">
      <button
        type="button"
        className="composer-undo-button"
        disabled={busy || disabled || !state.canUndo}
        onClick={() => void applyChange("undo")}
        title={latestEntry ? `Undo change to ${latestEntry.path}` : "Undo last workspace change"}
      >
        Undo
      </button>
      <button
        type="button"
        className="composer-undo-button"
        disabled={busy || disabled || !state.canRedo}
        onClick={() => void applyChange("redo")}
        title={redoEntry ? `Redo change to ${redoEntry.path}` : "Redo last undone change"}
      >
        Redo
      </button>
      {error ? (
        <span className="composer-undo-error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}
