import { useCallback, useEffect, useRef, useState } from "react";
import {
  getProjectMemoryState,
  updateProjectMemory
} from "../tauri";
import type {
  ProjectMemoryAction,
  ProjectMemoryItem,
  ProjectMemoryState
} from "../memoryManagementModel";

type MemorySettingsControllerOptions = {
  enabled: boolean;
  projectId: string | null;
  showSaved: (message?: string) => void;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function savedMessage(action: ProjectMemoryAction) {
  if (action === "delete") return "Memory deleted";
  if (action === "disable") return "Memory disabled";
  if (action === "enable") return "Memory enabled";
  if (action === "pin") return "Memory pinned";
  if (action === "unpin") return "Memory unpinned";
  return "Project requirement saved";
}

export function useMemorySettingsController({
  enabled,
  projectId,
  showSaved
}: MemorySettingsControllerOptions) {
  const [state, setState] = useState<ProjectMemoryState | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyMemoryId, setBusyMemoryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loadRequestRef = useRef(0);
  const projectIdRef = useRef(projectId);
  projectIdRef.current = projectId;

  const refresh = useCallback(async () => {
    const targetProjectId = projectId;
    const requestId = ++loadRequestRef.current;
    if (!enabled || !targetProjectId) {
      setState(null);
      setLoading(false);
      setError(null);
      return;
    }
    setState((current) =>
      current?.projectId === targetProjectId ? current : null
    );
    setLoading(true);
    setError(null);
    try {
      const next = await getProjectMemoryState(targetProjectId);
      if (
        requestId === loadRequestRef.current &&
        projectIdRef.current === targetProjectId &&
        next.projectId === targetProjectId
      ) {
        setState(next);
      }
    } catch (nextError) {
      if (
        requestId === loadRequestRef.current &&
        projectIdRef.current === targetProjectId
      ) {
        setError(errorMessage(nextError));
      }
    } finally {
      if (
        requestId === loadRequestRef.current &&
        projectIdRef.current === targetProjectId
      ) {
        setLoading(false);
      }
    }
  }, [enabled, projectId]);

  useEffect(() => {
    void refresh();
    return () => {
      loadRequestRef.current += 1;
    };
  }, [refresh]);

  useEffect(() => {
    setBusyMemoryId(null);
  }, [projectId]);

  const visibleState = state?.projectId === projectId ? state : null;

  const update = useCallback(
    async (
      item: ProjectMemoryItem,
      action: ProjectMemoryAction,
      confirmedContent?: string
    ) => {
      const targetProjectId = projectId;
      if (!enabled || !targetProjectId || busyMemoryId) return false;
      loadRequestRef.current += 1;
      setLoading(false);
      setBusyMemoryId(item.id);
      setError(null);
      try {
        const next = await updateProjectMemory({
          projectId: targetProjectId,
          memoryId: item.id,
          action,
          expectedItemRevision: item.itemRevision,
          expectedContentSha256: item.contentSha256,
          ...(action === "promote" ? { confirmedContent } : {})
        });
        if (
          projectIdRef.current !== targetProjectId ||
          next.projectId !== targetProjectId
        ) {
          return false;
        }
        setState(next);
        showSaved(savedMessage(action));
        return true;
      } catch (nextError) {
        if (projectIdRef.current === targetProjectId) {
          setError(errorMessage(nextError));
        }
        return false;
      } finally {
        if (projectIdRef.current === targetProjectId) {
          setBusyMemoryId(null);
        }
      }
    },
    [busyMemoryId, enabled, projectId, showSaved]
  );

  return {
    busyMemoryId,
    error,
    loading,
    projectId,
    refresh,
    state: visibleState,
    update
  };
}
