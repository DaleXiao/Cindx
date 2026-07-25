import { useCallback, useMemo, useState } from "react";
import type { InspectorTab } from "../components/Inspector";
import {
  answerWithRag,
  compactContext,
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
  type ContextState,
  type Phase5State,
  type Phase7State,
  type Phase8State
} from "../tauri";

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
  const [knowledgeGraphOpen, setKnowledgeGraphOpen] = useState(false);
  const [browserUrl, setBrowserUrl] = useState("https://example.com");
  const [browserTarget, setBrowserTarget] = useState("body");
  const [browserText, setBrowserText] = useState("hello");
  const [toolBusy, setToolBusy] = useState(false);
  const [ragBusy, setRagBusy] = useState(false);
  const [browserBusy, setBrowserBusy] = useState(false);
  const [contextBusy, setContextBusy] = useState(false);
  const [knowledgeError, setKnowledgeError] = useState<string | null>(null);

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

  const ensureKnowledgeIndex = useCallback(async () => {
    if (phase7 && phase7.stats.chunksIndexed > 0) return true;
    const indexed = await indexWorkspaceRag();
    setPhase7(indexed);
    if (!indexed.lastError) return true;
    reportError(indexed.lastError);
    return false;
  }, [phase7, reportError]);

  const handleIndexRag = useCallback(async () => {
    setRagBusy(true);
    setKnowledgeError(null);
    reportError(null);
    try {
      const next = await indexWorkspaceRag();
      setPhase7(next);
      showInspector("artifacts");
      reportError(next.lastError);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setKnowledgeError(message);
      reportError(message);
    } finally {
      setRagBusy(false);
    }
  }, [reportError, showInspector]);

  const runRagQuery = useCallback(
    async (mode: "search" | "answer") => {
      if (!ragQuery.trim()) return;
      setRagBusy(true);
      setKnowledgeError(null);
      reportError(null);
      try {
        if (!(await ensureKnowledgeIndex())) return;
        const next =
          mode === "search" ? await searchRag(ragQuery, 6) : await answerWithRag(ragQuery, 6);
        setPhase7(next);
        showInspector("artifacts");
        reportError(next.lastError);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setKnowledgeError(message);
        reportError(message);
      } finally {
        setRagBusy(false);
      }
    },
    [ensureKnowledgeIndex, ragQuery, reportError, showInspector]
  );

  const handleSearchRag = useCallback(() => runRagQuery("search"), [runRagQuery]);
  const handleAnswerWithRag = useCallback(() => runRagQuery("answer"), [runRagQuery]);

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
    ragBusy,
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
