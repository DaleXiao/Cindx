import { useCallback, useEffect, useMemo, useState } from "react";
import { getWorkspaceUndoState } from "../tauri";
import type { ChatMessageView } from "../tauriTypes";
import type { WorkspaceUndoState } from "../tauriTypes";
import {
  groupFileChangesByRun,
  lastMessageIdByRun,
  latestFileChangesRunId
} from "../fileChangesModel";
import { FileChangesPanel } from "../components/FileChangesPanel";
import type { SessionThreadSelection } from "../components/sessionThreadProjection";

/**
 * Session file-change history for the thread: fetched as runs finish, grouped
 * by the run that produced each change, and attached to the message that
 * closes that run so every turn carries its own change list.
 */
export function useSessionFileChanges(
  sessionId: string | null,
  status: string,
  messages: ChatMessageView[],
  messageId: (message: ChatMessageView, index: number) => string
) {
  const [undoState, setUndoState] = useState<WorkspaceUndoState | null>(null);

  const refreshUndoState = useCallback(async () => {
    if (!sessionId) {
      setUndoState(null);
      return;
    }
    try {
      setUndoState(await getWorkspaceUndoState(sessionId));
    } catch {
      // The panels are supplementary; a failed fetch leaves them hidden.
    }
  }, [sessionId]);

  useEffect(() => {
    void refreshUndoState();
  }, [refreshUndoState, status, messages.length]);

  const latestFileChangesRun = useMemo(
    () => latestFileChangesRunId(messages),
    [messages]
  );
  const fileChangeGroups = useMemo(
    () => groupFileChangesByRun(undoState?.entries ?? [], latestFileChangesRun),
    [undoState, latestFileChangesRun]
  );
  const fileChangeAttach = useMemo(
    () => lastMessageIdByRun(messages, messageId),
    [messages, messageId]
  );

  const working = status === "running" || status === "waiting_for_permission";

  /** Renders the run's file-change panel after the message that closes it. */
  const renderFileChangesPanel = (item: SessionThreadSelection) => {
    if (item.type !== "message") return null;
    const runId = item.message.runId;
    if (!runId) return null;
    const entries = fileChangeGroups.get(runId);
    if (!entries || fileChangeAttach.get(runId) !== item.id) return null;
    return (
      <FileChangesPanel
        sessionId={sessionId}
        entries={entries}
        canUndo={undoState?.canUndo ?? false}
        canRedo={undoState?.canRedo ?? false}
        isLatest={runId === latestFileChangesRun}
        working={working}
        onStateChanged={() => void refreshUndoState()}
      />
    );
  };

  return { undoState, refreshUndoState, renderFileChangesPanel };
}
