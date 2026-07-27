import { useCallback, useState } from "react";

type ComposerDrafts = Record<string, string>;
type SessionIdRef = { readonly current: string | null };

function updateDraft(
  current: ComposerDrafts,
  sessionId: string,
  value: string
): ComposerDrafts {
  if (current[sessionId] === value) return current;
  return { ...current, [sessionId]: value };
}

export function useComposerDrafts(
  activeSessionId: string | null,
  activeSessionIdRef: SessionIdRef
) {
  const [drafts, setDrafts] = useState<ComposerDrafts>({});
  const [focusRequest, setFocusRequest] = useState(0);

  const setActiveDraft = useCallback(
    (value: string) => {
      const sessionId = activeSessionIdRef.current ?? activeSessionId;
      if (!sessionId) return;
      setDrafts((current) => updateDraft(current, sessionId, value));
    },
    [activeSessionId, activeSessionIdRef]
  );

  const editActiveDraft = useCallback(
    (content: string) => {
      const sessionId = activeSessionIdRef.current;
      if (!sessionId) return;
      setDrafts((current) => updateDraft(current, sessionId, content));
      setFocusRequest((request) => request + 1);
    },
    [activeSessionIdRef]
  );

  const restoreDraftIfEmpty = useCallback((sessionId: string, value: string) => {
    setDrafts((current) =>
      updateDraft(current, sessionId, current[sessionId]?.trim() ? current[sessionId] : value)
    );
  }, []);

  const forgetDrafts = useCallback((sessionIds: string[]) => {
    if (sessionIds.length === 0) return;
    const deleted = new Set(sessionIds);
    setDrafts((current) => {
      if (!sessionIds.some((sessionId) => sessionId in current)) return current;
      const next = { ...current };
      deleted.forEach((sessionId) => delete next[sessionId]);
      return next;
    });
  }, []);

  return {
    value: activeSessionId ? drafts[activeSessionId] ?? "" : "",
    focusRequest,
    setActiveDraft,
    editActiveDraft,
    restoreDraftIfEmpty,
    forgetDrafts
  };
}
