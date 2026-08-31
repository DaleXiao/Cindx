import type { ChatMessageView, WorkspaceUndoEntryView } from "./tauriTypes";

/**
 * Groups session file-change entries by the agent run that produced them so
 * the thread can attach each change list to the turn that made it. Entries
 * without run attribution (persisted before attribution existed) attach to
 * the latest run instead of disappearing.
 */
export function groupFileChangesByRun(
  entries: WorkspaceUndoEntryView[],
  latestRunId: string | null
): Map<string | null, WorkspaceUndoEntryView[]> {
  const groups = new Map<string | null, WorkspaceUndoEntryView[]>();
  for (const entry of entries) {
    const runId = entry.runId ?? latestRunId;
    const group = groups.get(runId);
    if (group) group.push(entry);
    else groups.set(runId, [entry]);
  }
  return groups;
}

/**
 * The message id that closes each run (the last message carrying that runId),
 * which is where the run's file-change panel renders.
 */
export function lastMessageIdByRun(
  messages: ChatMessageView[],
  messageId: (message: ChatMessageView, index: number) => string
): Map<string, string> {
  const targets = new Map<string, string>();
  messages.forEach((message, index) => {
    const runId = message.runId;
    if (runId) targets.set(runId, messageId(message, index));
  });
  return targets;
}

/**
 * The run whose panel is "latest" (expanded, with undo/redo). A trailing user
 * message means a new turn has started, so the previous run's panel demotes
 * and collapses; otherwise the most recent attributed message wins.
 */
export function latestFileChangesRunId(
  messages: ChatMessageView[]
): string | null {
  const last = messages[messages.length - 1];
  if (!last || last.role === "user") return null;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const runId = messages[index].runId;
    if (runId) return runId;
  }
  return null;
}
