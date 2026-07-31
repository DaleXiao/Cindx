import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { InspectorTab } from "../components/Inspector";
import {
  answerWithRag,
  cancelRagOperation,
  compactContext,
  ensureWorkspaceKnowledge,
  getContextState,
  getPhase5State,
  getPhase7State,
  getPhase8State,
  indexWorkspaceRag,
  resolveBrowserPermission,
  resolveToolPermission,
  runBrowserTool,
  runTool,
  searchRag,
  subscribeToRagOperationProgress,
  type ContextState,
  type Phase5State,
  type Phase7State,
  type Phase8State,
  type RagOperationKind,
  type RagOperationProgress
} from "../tauri";
import {
  acceptRagOperationProgress,
  clearRagCancelAttemptIfCurrent,
  createRagCancelAttempt,
  createRagOperationId,
  shouldApplyRagOperationResult,
  waitForRagCancelOutcome,
  type RagCancelAttempt
} from "../ragOperationModel";

type KnowledgeToolingControllerOptions = {
  reportError: (message: string | null) => void;
  showInspector: (tab: InspectorTab) => void;
};

export function useKnowledgeToolingController({
  reportError,
  showInspector
}: KnowledgeToolingControllerOptions) {
  const [phase5, setPhase5] = useState<Phase5State | null>(null);
  const [phase7, setPhase7] = useState<Phase7State | null>(null);
  const [phase8, setPhase8] = useState<Phase8State | null>(null);
  const [contextState, setContextState] = useState<ContextState | null>(null);
  const [selectedTool, setSelectedTool] = useState("file.list");
  const [toolInput, setToolInput] = useState("path=.");
  const [ragQuery, setRagQuery] = useState("What is the Cindx MVP scope?");
  const [knowledgeGraphOpen, setKnowledgeGraphOpenState] = useState(false);
  const [browserUrl, setBrowserUrl] = useState("https://example.com");
  const [browserTarget, setBrowserTarget] = useState("body");
  const [browserText, setBrowserText] = useState("hello");
  const [toolBusy, setToolBusy] = useState(false);
  const [ragBusy, setRagBusy] = useState(false);
  const [ragCancelling, setRagCancelling] = useState(false);
  const [activeRagOperation, setActiveRagOperation] = useState<{
    id: string;
    kind: RagOperationKind;
  } | null>(null);
  const [ragProgress, setRagProgress] = useState<RagOperationProgress | null>(null);
  const [browserBusy, setBrowserBusy] = useState(false);
  const [contextBusy, setContextBusy] = useState(false);
  const [knowledgeError, setKnowledgeError] = useState<string | null>(null);
  const activeRagOperationIdRef = useRef<string | null>(null);
  const ragCancelAttemptRef = useRef<RagCancelAttempt | null>(null);

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | null = null;
    void subscribeToRagOperationProgress((progress) => {
      if (disposed) return;
      setRagProgress((current) =>
        acceptRagOperationProgress(activeRagOperationIdRef.current, current, progress)
      );
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unsubscribe = unlisten;
    });
    return () => {
      disposed = true;
      unsubscribe?.();
      const operationId = activeRagOperationIdRef.current;
      activeRagOperationIdRef.current = null;
      ragCancelAttemptRef.current?.settle({ accepted: true, signalError: null });
      ragCancelAttemptRef.current = null;
      if (operationId) void cancelRagOperation(operationId).catch(() => {});
    };
  }, []);

  const loadKnowledgeState = useCallback(() => {
    void getPhase5State()
      .then((state) => {
        setPhase5(state);
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
    void getPhase7State()
      .then((state) => {
        setPhase7(state);
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
    void getPhase8State()
      .then((state) => {
        setPhase8(state);
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
  }, [reportError]);

  const refreshWorkspaceKnowledge = useCallback(async (shouldApply?: () => boolean) => {
    const [nextPhase5, nextPhase7, nextPhase8, nextContext] = await Promise.all([
      getPhase5State(),
      getPhase7State(),
      getPhase8State(),
      getContextState()
    ]);
    if (shouldApply && !shouldApply()) return;
    setPhase5(nextPhase5);
    setPhase7(nextPhase7);
    setPhase8(nextPhase8);
    setContextState(nextContext);
  }, []);

  const handleRunTool = useCallback(async () => {
    if (!selectedTool) return;
    setToolBusy(true);
    reportError(null);
    try {
      const next = await runTool(selectedTool, toolInput);
      setPhase5(next);
      showInspector("artifacts");
      reportError(next.lastError);
    } finally {
      setToolBusy(false);
    }
  }, [reportError, selectedTool, showInspector, toolInput]);

  const handleResolveToolPermission = useCallback(
    async (requestId: string, decision: "allow_once" | "allow_for_session" | "deny") => {
      setToolBusy(true);
      reportError(null);
      try {
        const next = await resolveToolPermission(requestId, decision);
        setPhase5(next);
        reportError(next.lastError);
      } finally {
        setToolBusy(false);
      }
    },
    [reportError]
  );

  const setKnowledgeGraphOpen = useCallback(
    (open: boolean) => {
      setKnowledgeGraphOpenState(open);
      if (!open || activeRagOperationIdRef.current) return;
      setRagBusy(true);
      setKnowledgeError(null);
      void ensureWorkspaceKnowledge()
        .then((state) => {
          setPhase7(state);
          if (state.lastError) {
            setKnowledgeError(state.lastError);
            reportError(state.lastError);
          }
        })
        .catch((error) => {
          const message = error instanceof Error ? error.message : String(error);
          setKnowledgeError(message);
          reportError(message);
        })
        .finally(() => setRagBusy(false));
    },
    [reportError]
  );

  const runRagOperation = useCallback(async (
    kind: RagOperationKind,
    operation: (operationId: string) => Promise<Phase7State>
  ) => {
    if (activeRagOperationIdRef.current) return;
    const operationId = createRagOperationId(kind);
    activeRagOperationIdRef.current = operationId;
    ragCancelAttemptRef.current = null;
    setActiveRagOperation({ id: operationId, kind });
    setRagProgress(null);
    setRagCancelling(false);
    setRagBusy(true);
    setKnowledgeError(null);
    reportError(null);
    try {
      const next = await operation(operationId);
      const cancelOutcome = await waitForRagCancelOutcome(
        operationId,
        ragCancelAttemptRef.current
      );
      if (
        !shouldApplyRagOperationResult(
          activeRagOperationIdRef.current,
          operationId,
          cancelOutcome
        )
      ) {
        return;
      }
      setPhase7(next);
      showInspector("artifacts");
      if (cancelOutcome.signalError) {
        setKnowledgeError(cancelOutcome.signalError);
        reportError(cancelOutcome.signalError);
      } else {
        reportError(next.lastError);
      }
    } catch (error) {
      const cancelOutcome = await waitForRagCancelOutcome(
        operationId,
        ragCancelAttemptRef.current
      );
      if (
        !shouldApplyRagOperationResult(
          activeRagOperationIdRef.current,
          operationId,
          cancelOutcome
        )
      ) {
        return;
      }
      const message = cancelOutcome.signalError ??
        (error instanceof Error ? error.message : String(error));
      setKnowledgeError(message);
      reportError(message);
    } finally {
      if (activeRagOperationIdRef.current === operationId) {
        activeRagOperationIdRef.current = null;
        ragCancelAttemptRef.current = null;
        setActiveRagOperation(null);
        setRagProgress(null);
        setRagCancelling(false);
        setRagBusy(false);
      }
    }
  }, [reportError, showInspector]);

  const handleIndexRag = useCallback(
    () => runRagOperation("index", (operationId) => indexWorkspaceRag(operationId)),
    [runRagOperation]
  );

  const runRagQuery = useCallback(
    async (mode: "search" | "answer") => {
      const query = ragQuery.trim();
      if (!query) return;
      await runRagOperation(mode, (operationId) =>
        mode === "search"
          ? searchRag(operationId, query, 6)
          : answerWithRag(operationId, query, 6)
      );
    },
    [ragQuery, runRagOperation]
  );

  const handleSearchRag = useCallback(() => runRagQuery("search"), [runRagQuery]);
  const handleAnswerWithRag = useCallback(() => runRagQuery("answer"), [runRagQuery]);

  const handleCancelRag = useCallback(async () => {
    const operationId = activeRagOperationIdRef.current;
    if (!operationId || !ragProgress || ragProgress.status !== "running") return;
    if (ragCancelAttemptRef.current?.operationId === operationId) return;
    const attempt = createRagCancelAttempt(operationId);
    ragCancelAttemptRef.current = attempt;
    setRagCancelling(true);
    try {
      const signalled = await cancelRagOperation(operationId);
      attempt.settle({ accepted: signalled, signalError: null });
      if (activeRagOperationIdRef.current === operationId && !signalled) {
        ragCancelAttemptRef.current = clearRagCancelAttemptIfCurrent(
          ragCancelAttemptRef.current,
          attempt
        );
        setRagCancelling(false);
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      attempt.settle({ accepted: false, signalError: message });
      if (activeRagOperationIdRef.current !== operationId) return;
      ragCancelAttemptRef.current = clearRagCancelAttemptIfCurrent(
        ragCancelAttemptRef.current,
        attempt
      );
      setRagCancelling(false);
      setKnowledgeError(message);
      reportError(message);
    }
  }, [ragProgress, reportError]);

  const handleCompactContext = useCallback(async () => {
    setContextBusy(true);
    reportError(null);
    try {
      const next = await compactContext();
      setContextState(next);
      showInspector("context");
      reportError(next.lastError);
    } finally {
      setContextBusy(false);
    }
  }, [reportError, showInspector]);

  const handleRunBrowserTool = useCallback(
    async (toolName: string) => {
      const value = browserUrl.trim();
      if (!value && toolName !== "browser.tabs" && toolName !== "browser.select_tab") return;
      let input = `url=${value}\noutput_dir=.cindx/browser-captures`;
      if (toolName === "web.search") input = `query=${value}`;
      else if (toolName === "browser.tabs") input = "";
      else if (toolName === "browser.select_tab") input = `tab_id=${browserTarget.trim()}`;
      else if (toolName === "browser.click") {
        input = `url=${value}\nselector=${browserTarget.trim() || "body"}\noutput_dir=.cindx/browser-actions`;
      } else if (toolName === "browser.type") {
        input = `url=${value}\nselector=${browserTarget.trim() || "body"}\ntext=${browserText}\noutput_dir=.cindx/browser-actions`;
      } else if (toolName === "browser.scroll") {
        input = `url=${value}\ndelta_y=600\noutput_dir=.cindx/browser-actions`;
      }
      setBrowserBusy(true);
      reportError(null);
      try {
        const next = await runBrowserTool(toolName, input);
        setPhase8(next);
        showInspector("artifacts");
        reportError(next.lastError);
      } finally {
        setBrowserBusy(false);
      }
    },
    [browserTarget, browserText, browserUrl, reportError, showInspector]
  );

  const handleResolveBrowserPermission = useCallback(
    async (requestId: string, decision: "allow_once" | "allow_for_session" | "deny") => {
      setBrowserBusy(true);
      reportError(null);
      try {
        const next = await resolveBrowserPermission(requestId, decision);
        setPhase8(next);
        reportError(next.lastError);
      } finally {
        setBrowserBusy(false);
      }
    },
    [reportError]
  );

  const selectedToolSpec = phase5?.tools.find((tool) => tool.name === selectedTool);
  const ragStats = phase7?.stats ?? { filesIndexed: 0, chunksIndexed: 0, indexedAtMs: 0 };
  const ragSources = phase7?.sources ?? [];
  const browserObservations = phase8?.observations ?? [];
  const toolResults = phase5?.results ?? [];
  const contextCheckpoint = contextState?.checkpoint ?? null;
  const toolApprovals = useMemo(() => phase5?.pendingApprovals ?? [], [phase5?.pendingApprovals]);
  const browserApprovals = useMemo(
    () => phase8?.pendingApprovals ?? [],
    [phase8?.pendingApprovals]
  );

  return {
    browserApprovals,
    browserBusy,
    browserObservations,
    browserTarget,
    browserText,
    browserUrl,
    contextBusy,
    contextCheckpoint,
    contextState,
    handleAnswerWithRag,
    handleCancelRag,
    handleCompactContext,
    handleIndexRag,
    handleResolveBrowserPermission,
    handleResolveToolPermission,
    handleRunBrowserTool,
    handleRunTool,
    handleSearchRag,
    knowledgeError,
    knowledgeGraphOpen,
    loadKnowledgeState,
    phase5,
    phase7,
    phase8,
    activeRagOperation,
    ragBusy,
    ragCancelling,
    ragProgress,
    ragQuery,
    ragSources,
    ragStats,
    refreshWorkspaceKnowledge,
    selectedTool,
    selectedToolSpec,
    setBrowserTarget,
    setBrowserText,
    setBrowserUrl,
    setContextState,
    setKnowledgeGraphOpen,
    setRagQuery,
    setSelectedTool,
    setToolInput,
    toolApprovals,
    toolBusy,
    toolInput,
    toolResults
  };
}
