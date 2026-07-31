import { useCallback, useState } from "react";
import type { AgentAttachment } from "../tauri";
import {
  removeAgentAttachment,
  stageAgentAttachments
} from "../tauri";

type ComposerAttachmentsInput = {
  reportError: (message: string | null) => void;
  sessionId: string | null;
};

export function useComposerAttachments({
  reportError,
  sessionId
}: ComposerAttachmentsInput) {
  const [drafts, setDrafts] = useState<Record<string, AgentAttachment[]>>({});
  const [busySessionIds, setBusySessionIds] = useState<Set<string>>(() => new Set());
  const attachments = sessionId ? drafts[sessionId] ?? [] : [];
  const busy = Boolean(sessionId && busySessionIds.has(sessionId));

  const pick = useCallback(
    async (files: File[]) => {
      if (!sessionId || files.length === 0) return;
      const existing = drafts[sessionId] ?? [];
      const available = Math.max(0, 10 - existing.length);
      if (files.length > available) {
        reportError("A message can include at most 10 attachments.");
        return;
      }
      setBusySessionIds((current) => new Set(current).add(sessionId));
      reportError(null);
      try {
        const staged = await stageAgentAttachments(sessionId, files);
        setDrafts((current) => ({
          ...current,
          [sessionId]: [...(current[sessionId] ?? []), ...staged]
        }));
      } catch (error) {
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusySessionIds((current) => {
          const next = new Set(current);
          next.delete(sessionId);
          return next;
        });
      }
    },
    [drafts, reportError, sessionId]
  );

  const remove = useCallback(
    (attachment: AgentAttachment) => {
      if (!sessionId) return;
      setDrafts((current) => ({
        ...current,
        [sessionId]: (current[sessionId] ?? []).filter(
          (item) => item.id !== attachment.id
        )
      }));
      void removeAgentAttachment(sessionId, attachment.path).catch((error) => {
        reportError(error instanceof Error ? error.message : String(error));
      });
    },
    [reportError, sessionId]
  );

  const clear = useCallback((targetSessionId: string) => {
    setDrafts((current) => ({ ...current, [targetSessionId]: [] }));
  }, []);

  const restoreIfEmpty = useCallback(
    (targetSessionId: string, previous: AgentAttachment[]) => {
      if (previous.length === 0) return;
      setDrafts((current) =>
        (current[targetSessionId] ?? []).length > 0
          ? current
          : { ...current, [targetSessionId]: previous }
      );
    },
    []
  );

  const forget = useCallback((sessionIds: string[]) => {
    if (sessionIds.length === 0) return;
    const deleted = new Set(sessionIds);
    setDrafts((current) => withoutKeys(current, deleted));
    setBusySessionIds(
      (current) => new Set([...current].filter((id) => !deleted.has(id)))
    );
  }, []);

  return {
    attachments,
    busy,
    clear,
    forget,
    pick,
    remove,
    restoreIfEmpty
  };
}

function withoutKeys<Value>(current: Record<string, Value>, keys: Set<string>) {
  const next = { ...current };
  keys.forEach((key) => delete next[key]);
  return next;
}
