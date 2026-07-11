import {
  Activity,
  ArrowLeft,
  ArchiveRestore,
  BookOpen,
  Bot,
  Cable,
  CheckCircle2,
  ChevronRight,
  Clock3,
  Database,
  FileText,
  Globe2,
  Info,
  KeyRound,
  LayoutDashboard,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Play,
  RefreshCw,
  Save,
  Search,
  Send,
  ShieldCheck,
  ShieldQuestion,
  TerminalSquare,
  Trash2,
  TriangleAlert,
  XCircle
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { Inspector, type InspectorTab } from "./components/Inspector";
import { Composer } from "./components/Composer";
import { DisclosureTriangle } from "./components/DisclosureTriangle";
import { Sidebar, type WorkspaceView } from "./components/Sidebar";
import { TraceStatusIcon } from "./components/TraceStatusIcon";
import {
  SessionThread,
  type SessionThreadSelection
} from "./components/SessionThread";
import {
  AgentState,
  AgentTraceState,
  AgentTraceStepView,
  answerWithRag,
  archiveSession,
  cancelAgentTask,
  compactContext,
  ContextState,
  createProject,
  createSession,
  deleteSession,
  DESKTOP_VERSION,
  exportAgentTraceJsonl,
  getAgentState,
  getAgentTraceState,
  getContextState,
  getPhase3State,
  getPhase4State,
  getPhase5State,
  getPhase6State,
  getPhase7State,
  getPhase8State,
  getProjectSessionState,
  getRuntimeStatus,
  getSidecarState,
  getMcpState,
  getSkillState,
  indexWorkspaceRag,
  forkSession,
  listProviderModels,
  Phase3State,
  Phase4State,
  Phase5State,
  Phase6State,
  Phase7State,
  Phase8State,
  McpServerConfig,
  McpState,
  ProviderConfigInput,
  ProviderConfigState,
  ProjectSessionState,
  requestMockPermission,
  resolveBrowserPermission,
  resolveAgentPermission,
  resolveToolPermission,
  resolvePermission,
  RuntimeStatus,
  retryAgentTask,
  renameSession,
  restoreSession,
  runAgentTask,
  runBrowserTool,
  runTool,
  runOrchestration,
  saveProviderConfig,
  saveSidecarConfig,
  refreshMcpServer,
  refreshSkills,
  removeMcpServer,
  saveSkillPreference,
  SkillState,
  updateMcpServerPolicy,
  upsertMcpServer,
  saveWorkspaceRoot,
  searchRag,
  selectProject,
  selectSession,
  SidecarState,
  subscribeToModelStream
} from "./tauri";

const appIconUrl = new URL("../src-tauri/icons/icon.png", import.meta.url).href;

function providerDraftFromState(provider: ProviderConfigState): ProviderConfigInput {
  return {
    baseUrl: provider.baseUrl,
    apiKey: "",
    model: provider.model,
    plannerModel: provider.plannerModel,
    executorModel: provider.executorModel,
    reviewerModel: provider.reviewerModel,
    summarizerModel: provider.summarizerModel,
    embeddingModel: provider.embeddingModel,
    collaborationPolicy: provider.collaborationPolicy,
    contextWindowTokens: provider.contextWindowTokens,
    agentSystemPrompt: provider.agentSystemPrompt
  };
}

function TraceIcon({ step }: { step: AgentTraceStepView }) {
  if (step.kind === "tool") return <TerminalSquare aria-hidden="true" />;
  if (step.kind === "permission") return <ShieldCheck aria-hidden="true" />;
  if (step.kind === "model") return <Activity aria-hidden="true" />;
  if (step.kind === "error") return <TriangleAlert aria-hidden="true" />;
  return <FileText aria-hidden="true" />;
}

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "local";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(timestampMs);
}

function formatDuration(durationMs: number | null) {
  if (durationMs == null) return "live";
  if (durationMs < 1000) return `${durationMs} ms`;
  return `${(durationMs / 1000).toFixed(1)} s`;
}

function formatTokenCount(tokens: number) {
  if (tokens < 1000) return String(tokens);
  if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(tokens < 10_000 ? 1 : 0)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}

function isAutoSessionName(name: string) {
  return ["runtime session", "new session", "untitled session", "session"].includes(
    name.trim().toLowerCase()
  );
}

function sessionTitleFromPrompt(prompt: string) {
  const compact = prompt.replace(/\s+/g, " ").trim();
  return [...compact].slice(0, 36).join("").trimEnd() || "New Session";
}

function latestTraceStep(turns: { steps: AgentTraceStepView[] }[]) {
  const steps = turns.flatMap((turn) => turn.steps);
  return steps.length > 0 ? steps[steps.length - 1] : null;
}

function ModelSelect({
  label,
  value,
  options,
  onChange
}: {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
}) {
  return (
    <label>
      <span>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)}>
        {options.map((model) => (
          <option value={model} key={model}>
            {model}
          </option>
        ))}
      </select>
    </label>
  );
}

const settingsCategories = [
  { id: "runtime", label: "Runtime", description: "Workspace and local services" },
  { id: "sessions", label: "Sessions", description: "Archived session recovery" },
  { id: "models", label: "Models", description: "Providers and orchestration" },
  { id: "agent", label: "Agent", description: "System instructions and behavior" },
  { id: "knowledge", label: "Knowledge", description: "Context, memory, and retrieval" },
  { id: "tools", label: "Tools", description: "Browser and local tool controls" },
  { id: "mcp", label: "MCP", description: "External tool servers and catalogs" },
  { id: "skills", label: "Skills", description: "Reusable agent instructions and resources" },
  { id: "permissions", label: "Permissions", description: "Approvals and review history" },
  { id: "about", label: "About", description: "Version and application information" }
] as const;

type SettingsCategory = (typeof settingsCategories)[number]["id"];

function SettingsCategoryIcon({ category }: { category: SettingsCategory }) {
  if (category === "runtime") return <LayoutDashboard aria-hidden="true" />;
  if (category === "sessions") return <ArchiveRestore aria-hidden="true" />;
  if (category === "models") return <KeyRound aria-hidden="true" />;
  if (category === "agent") return <Bot aria-hidden="true" />;
  if (category === "knowledge") return <Database aria-hidden="true" />;
  if (category === "tools") return <TerminalSquare aria-hidden="true" />;
  if (category === "mcp") return <Cable aria-hidden="true" />;
  if (category === "skills") return <BookOpen aria-hidden="true" />;
  if (category === "permissions") return <ShieldCheck aria-hidden="true" />;
  return <Info aria-hidden="true" />;
}

export function App() {
  const compactDesktop =
    typeof window !== "undefined" && window.matchMedia("(max-width: 1180px)").matches;
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [activeView, setActiveView] = useState<WorkspaceView>("timeline");
  const [workspaceViewBeforeSettings, setWorkspaceViewBeforeSettings] =
    useState<Exclude<WorkspaceView, "settings">>("timeline");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory | null>(null);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("details");
  const [inspectorOpen, setInspectorOpen] = useState(!compactDesktop);
  const [inspectorOpenBeforeSettings, setInspectorOpenBeforeSettings] = useState(!compactDesktop);
  const [inspectorWidth, setInspectorWidth] = useState(320);
  const [phase3, setPhase3] = useState<Phase3State | null>(null);
  const [phase4, setPhase4] = useState<Phase4State | null>(null);
  const [phase5, setPhase5] = useState<Phase5State | null>(null);
  const [, setPhase6] = useState<Phase6State | null>(null);
  const [phase7, setPhase7] = useState<Phase7State | null>(null);
  const [phase8, setPhase8] = useState<Phase8State | null>(null);
  const [contextState, setContextState] = useState<ContextState | null>(null);
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [agentTraceState, setAgentTraceState] = useState<AgentTraceState | null>(null);
  const [selectedThreadItem, setSelectedThreadItem] =
    useState<SessionThreadSelection | null>(null);
  const [selectedTraceStepId, setSelectedTraceStepId] = useState<string | null>(null);
  const [sidecarState, setSidecarState] = useState<SidecarState | null>(null);
  const [mcpState, setMcpState] = useState<McpState | null>(null);
  const [skillState, setSkillState] = useState<SkillState | null>(null);
  const [projectSessionState, setProjectSessionState] = useState<ProjectSessionState | null>(null);
  const [sidecarDraft, setSidecarDraft] = useState({
    browserPath: "",
    computerPath: "",
    autoConfigure: true
  });
  const [providerDraft, setProviderDraft] = useState<ProviderConfigInput | null>(null);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [newProjectName, setNewProjectName] = useState("");
  const [projectCreateOpen, setProjectCreateOpen] = useState(false);
  const [sidebarSearchOpen, setSidebarSearchOpen] = useState(false);
  const [sidebarQuery, setSidebarQuery] = useState("");
  const [composerDrafts, setComposerDrafts] = useState<Record<string, string>>({});
  const [composerFocusRequest, setComposerFocusRequest] = useState(0);
  const [streamAnswer, setStreamAnswer] = useState("");
  const [providerModels, setProviderModels] = useState<string[]>([]);
  const [providerModelsBusy, setProviderModelsBusy] = useState(false);
  const [providerModelsError, setProviderModelsError] = useState<string | null>(null);
  const [selectedTool, setSelectedTool] = useState("file.list");
  const [toolInput, setToolInput] = useState("path=.");
  const [orchestrationPolicy, setOrchestrationPolicy] = useState("auto_router");
  const [orchestrationPrompt, setOrchestrationPrompt] = useState("Inspect this project and suggest the next safe MVP step.");
  const [ragQuery, setRagQuery] = useState("What is the Cindx MVP scope?");
  const [browserUrl, setBrowserUrl] = useState("https://example.com");
  const [browserTarget, setBrowserTarget] = useState("body");
  const [browserText, setBrowserText] = useState("hello");
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [providerBusy, setProviderBusy] = useState(false);
  const [toolBusy, setToolBusy] = useState(false);
  const [orchestrationBusy, setOrchestrationBusy] = useState(false);
  const [ragBusy, setRagBusy] = useState(false);
  const [browserBusy, setBrowserBusy] = useState(false);
  const [contextBusy, setContextBusy] = useState(false);
  const [traceBusy, setTraceBusy] = useState(false);
  const [sidecarBusy, setSidecarBusy] = useState(false);
  const [mcpBusy, setMcpBusy] = useState(false);
  const [skillBusy, setSkillBusy] = useState(false);
  const [mcpDraft, setMcpDraft] = useState({
    name: "",
    command: "",
    args: "",
    envKey: "",
    envValue: ""
  });
  const [projectSessionBusy, setProjectSessionBusy] = useState(false);
  const [busySessionIds, setBusySessionIds] = useState<Set<string>>(() => new Set());
  const [sessionStatusOverrides, setSessionStatusOverrides] = useState<Record<string, string>>({});
  const activeSessionIdRef = useRef<string | null>(null);
  const [composerError, setComposerError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let deferredLoadTimer: number | null = null;
    let deferredIdleCallback: number | null = null;
    let unlisten = () => {};

    subscribeToModelStream((payload) => {
      if (payload.done) {
        if (payload.error) setComposerError(payload.error);
        return;
      }
      setStreamAnswer((current) => `${current}${payload.delta}`);
    }).then((handler) => {
      if (disposed) handler();
      else unlisten = handler;
    });

    const coreRequests = [
      getRuntimeStatus().then((state) => {
        setRuntime(state);
        setWorkspaceDraft(state.workspaceRoot);
      }),
      getProjectSessionState().then((state) => {
        setProjectSessionState(state);
        setComposerError((current) => current ?? state.lastError);
      }),
      getAgentState().then((state) => {
        setAgentState(state);
        if (state.sessionId) updateSessionStatus(state.sessionId, state.status);
        setComposerError((current) => current ?? state.lastError);
      })
    ];

    const loadDeferredState = () => {
      if (disposed) return;
      getSidecarState().then((state) => {
        setSidecarState(state);
        setSidecarDraft({
          browserPath: state.browser.path,
          computerPath: state.computer.path,
          autoConfigure: state.autoConfigure
        });
        setComposerError((current) => current ?? state.lastError);
      });
      getMcpState().then((state) => {
        setMcpState(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getSkillState().then((state) => {
        setSkillState(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getPhase3State().then(setPhase3);
      getPhase4State().then((state) => {
        setPhase4(state);
        setProviderDraft(providerDraftFromState(state.provider));
        setComposerError(state.lastError);
      });
      getPhase5State().then((state) => {
        setPhase5(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getPhase6State().then((state) => {
        setPhase6(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getPhase7State().then((state) => {
        setPhase7(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getPhase8State().then((state) => {
        setPhase8(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getContextState().then((state) => {
        setContextState(state);
        setComposerError((current) => current ?? state.lastError);
      });
      getAgentTraceState().then((state) => {
        setAgentTraceState(state);
        const latestStep = latestTraceStep(state.turns);
        setSelectedTraceStepId(latestStep?.id ?? null);
        setComposerError((current) => current ?? state.lastError);
      });
    };

    void Promise.allSettled(coreRequests).then(() => {
      if (disposed) return;
      if (typeof window.requestIdleCallback === "function") {
        deferredIdleCallback = window.requestIdleCallback(loadDeferredState, { timeout: 1000 });
      } else {
        deferredLoadTimer = window.setTimeout(loadDeferredState, 250);
      }
    });

    return () => {
      disposed = true;
      if (deferredLoadTimer !== null) window.clearTimeout(deferredLoadTimer);
      if (deferredIdleCallback !== null) window.cancelIdleCallback(deferredIdleCallback);
      unlisten();
    };
  }, []);

  const statusText = useMemo(() => {
    if (!runtime) return "Connecting";
    if (runtime.kernelStatus === "kernel bridge online") return "Ready";
    return runtime.kernelStatus;
  }, [runtime]);

  const activeProject = useMemo(
    () => projectSessionState?.projects.find((project) => project.active) ?? null,
    [projectSessionState?.projects]
  );

  const activeSession = useMemo(
    () => projectSessionState?.sessions.find((session) => session.active) ?? null,
    [projectSessionState?.sessions]
  );

  useEffect(() => {
    activeSessionIdRef.current = activeSession?.id ?? null;
  }, [activeSession?.id]);

  const composerDraft = activeSession ? composerDrafts[activeSession.id] ?? "" : "";

  function setActiveComposerDraft(value: string) {
    const sessionId = activeSessionIdRef.current ?? activeSession?.id;
    if (!sessionId) return;
    setComposerDrafts((current) => ({ ...current, [sessionId]: value }));
  }

  const normalizedSidebarQuery = sidebarQuery.trim().toLowerCase();

  const sessions = useMemo(
    () =>
      (projectSessionState?.sessions ?? [])
        .filter((session) => {
          if (session.archived) return false;
          if (activeProject && session.projectId !== activeProject.id) return false;
          if (!normalizedSidebarQuery) return true;
          return `${session.name} ${session.detail}`
            .toLowerCase()
            .includes(normalizedSidebarQuery);
        })
        .map((session) =>
          busySessionIds.has(session.id)
            ? { ...session, status: "Working" }
            : sessionStatusOverrides[session.id]
              ? { ...session, status: sessionStatusOverrides[session.id] }
              : session
        ),
    [
      activeProject,
      busySessionIds,
      normalizedSidebarQuery,
      projectSessionState?.sessions,
      sessionStatusOverrides
    ]
  );

  const archivedSessions = useMemo(
    () =>
      (projectSessionState?.sessions ?? [])
        .filter((session) => session.archived)
        .sort((left, right) => (right.archivedAtMs ?? 0) - (left.archivedAtMs ?? 0)),
    [projectSessionState?.sessions]
  );

  const projects = useMemo(
    () =>
      (projectSessionState?.projects ?? []).filter((project) => {
        if (!normalizedSidebarQuery) return true;
        return `${project.name} ${project.detail} ${project.root}`
          .toLowerCase()
          .includes(normalizedSidebarQuery);
      }),
    [normalizedSidebarQuery, projectSessionState?.projects]
  );

  const permissionRows = phase3?.permissions ?? [];
  const toolApprovals = phase5?.pendingApprovals ?? [];
  const browserApprovals = phase8?.pendingApprovals ?? [];
  const agentApprovals = agentState?.pendingApprovals ?? [];
  const externalApprovals = [
    ...toolApprovals.map((approval) => ({ ...approval, source: "tool" as const })),
    ...browserApprovals.map((approval) => ({ ...approval, source: "browser" as const }))
  ];
  const hasPendingPermission =
    permissionRows.some((permission) => permission.status === "pending") ||
    toolApprovals.length > 0 ||
    browserApprovals.length > 0 ||
    agentApprovals.length > 0;
  const toolResults = phase5?.results ?? [];
  const ragStats = phase7?.stats ?? { filesIndexed: 0, chunksIndexed: 0, indexedAtMs: 0 };
  const ragSources = phase7?.sources ?? [];
  const browserObservations = phase8?.observations ?? [];
  const contextCheckpoint = contextState?.checkpoint ?? null;
  const activeSessionBusy = Boolean(activeSession && busySessionIds.has(activeSession.id));
  const agentCanCancel = Boolean(agentState?.canCancel || activeSessionBusy);
  const agentCanRetry = Boolean(agentState?.canRetry);
  const agentWorking = Boolean(activeSessionBusy || agentState?.status === "running");
  const traceTurns = agentTraceState?.turns ?? [];
  const traceSteps = traceTurns.flatMap((turn) => turn.steps);
  const selectedTraceStep =
    traceSteps.find((step) => step.id === selectedTraceStepId) ??
    (traceSteps.length > 0 ? traceSteps[traceSteps.length - 1] : null);
  const activeSettingsCategory =
    settingsCategories.find((category) => category.id === settingsCategory) ?? null;
  const providerModelOptions = useMemo(() => {
    const configured = providerDraft
      ? [
          providerDraft.model,
          providerDraft.plannerModel,
          providerDraft.executorModel,
          providerDraft.reviewerModel,
          providerDraft.summarizerModel,
          providerDraft.embeddingModel
        ]
      : [];
    return [...new Set([...providerModels, ...configured].filter(Boolean))].sort();
  }, [providerDraft, providerModels]);
  const collaborationModelCount = providerDraft
    ? new Set([
        providerDraft.plannerModel,
        providerDraft.executorModel,
        providerDraft.reviewerModel,
        providerDraft.summarizerModel
      ]).size
    : 0;

  const selectedToolSpec = phase5?.tools.find((tool) => tool.name === selectedTool);

  function markSessionBusy(sessionId: string, busy: boolean) {
    setBusySessionIds((current) => {
      const next = new Set(current);
      if (busy) next.add(sessionId);
      else next.delete(sessionId);
      return next;
    });
  }

  function updateSessionStatus(sessionId: string, status: AgentState["status"]) {
    setSessionStatusOverrides((current) => {
      const next = { ...current };
      if (status === "waiting_for_permission") next[sessionId] = "Review";
      else if (status === "running") next[sessionId] = "Working";
      else delete next[sessionId];
      return next;
    });
  }

  async function refreshAgentTrace(
    selectLatest = false,
    sessionId = activeSessionIdRef.current
  ) {
    const next = await getAgentTraceState(sessionId);
    if (sessionId === activeSessionIdRef.current) {
      setAgentTraceState(next);
      if (selectLatest) {
        const latestStep = latestTraceStep(next.turns);
        setSelectedTraceStepId(latestStep?.id ?? null);
      }
      setComposerError((current) => current ?? next.lastError);
    }
    return next;
  }

  async function handleRequestPermission() {
    setPermissionBusy(true);
    try {
      setPhase3(await requestMockPermission());
    } finally {
      setPermissionBusy(false);
    }
  }

  async function handleResolvePermission(
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    setPermissionBusy(true);
    try {
      setPhase3(await resolvePermission(requestId, decision));
    } finally {
      setPermissionBusy(false);
    }
  }

  async function handleSaveProviderConfig() {
    if (!providerDraft) return;
    setProviderBusy(true);
    setComposerError(null);
    try {
      const next = await saveProviderConfig(providerDraft);
      setPhase4(next);
      setProviderDraft(providerDraftFromState(next.provider));
    } finally {
      setProviderBusy(false);
    }
  }

  async function handleLoadProviderModels() {
    if (!providerDraft || providerModelsBusy) return;
    setProviderModelsBusy(true);
    setProviderModelsError(null);
    try {
      const next = await listProviderModels({
        baseUrl: providerDraft.baseUrl,
        apiKey: providerDraft.apiKey
      });
      setProviderModels(next.models);
      setProviderModelsError(next.lastError);
    } finally {
      setProviderModelsBusy(false);
    }
  }

  async function handleSaveWorkspace() {
    const nextPath = workspaceDraft.trim();
    if (!nextPath) return;
    setWorkspaceBusy(true);
    setComposerError(null);
    try {
      const next = await saveWorkspaceRoot(nextPath);
      setRuntime(next);
      setWorkspaceDraft(next.workspaceRoot);
      setProjectSessionState(await getProjectSessionState());
      setPhase5(await getPhase5State());
      setPhase7(await getPhase7State());
      setPhase8(await getPhase8State());
      setContextState(await getContextState());
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function refreshWorkspaceAfterProjectSession(nextState: ProjectSessionState) {
    setProjectSessionState(nextState);
    setComposerError(nextState.lastError);
    const nextRuntime = await getRuntimeStatus();
    setRuntime(nextRuntime);
    setWorkspaceDraft(nextRuntime.workspaceRoot);
    setPhase5(await getPhase5State());
    setPhase7(await getPhase7State());
    setPhase8(await getPhase8State());
    setContextState(await getContextState());
    const nextAgentState = await getAgentState(nextState.activeSessionId);
    const nextTraceState = await getAgentTraceState(nextState.activeSessionId);
    setAgentState(nextAgentState);
    updateSessionStatus(nextState.activeSessionId, nextAgentState.status);
    setAgentTraceState(nextTraceState);
    setSelectedTraceStepId(latestTraceStep(nextTraceState.turns)?.id ?? null);
    setSelectedThreadItem(null);
    setStreamAnswer("");
  }

  function showWorkspaceView(view: Exclude<WorkspaceView, "settings">) {
    const leavingSettings = activeView === "settings";
    setWorkspaceViewBeforeSettings(view);
    setActiveView(view);
    if (view === "trace") {
      setInspectorTab("details");
      setInspectorOpen(true);
    } else if (leavingSettings) {
      setInspectorOpen(inspectorOpenBeforeSettings);
    }
  }

  function showTimelineView() {
    showWorkspaceView("timeline");
  }

  function handleWorkspaceViewChange(view: WorkspaceView) {
    if (view === "settings") {
      if (activeView === "settings") {
        showWorkspaceView(workspaceViewBeforeSettings);
        return;
      }
      setWorkspaceViewBeforeSettings(activeView);
      setSettingsCategory(null);
      setInspectorOpenBeforeSettings(inspectorOpen);
      setActiveView("settings");
      setInspectorOpen(false);
      return;
    }
    showWorkspaceView(view);
  }

  async function handleCreateProject() {
    const name = newProjectName.trim();
    const root = workspaceDraft.trim() || runtime?.workspaceRoot || "";
    if (!name || !root) return;
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await createProject(name, root);
      setNewProjectName("");
      setProjectCreateOpen(false);
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleCreateSession() {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await createSession("New Session", activeProject?.id ?? undefined);
      await refreshWorkspaceAfterProjectSession(next);
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleSelectProject(projectId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await selectProject(projectId);
      await refreshWorkspaceAfterProjectSession(next);
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleSelectSession(sessionId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await selectSession(sessionId);
      await refreshWorkspaceAfterProjectSession(next);
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleForkSession(sessionId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await forkSession(sessionId));
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleRenameSession(sessionId: string, name: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await renameSession(sessionId, name));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleArchiveSession(sessionId: string) {
    if (busySessionIds.has(sessionId) || sessionStatusOverrides[sessionId] === "Review") {
      setComposerError("Stop the running session before archiving it.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await archiveSession(sessionId));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleRestoreSession(sessionId: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await restoreSession(sessionId);
      setProjectSessionState(next);
      setComposerError(next.lastError);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleDeleteSession(sessionId: string) {
    if (busySessionIds.has(sessionId) || sessionStatusOverrides[sessionId] === "Review") {
      setComposerError("Stop the running session before deleting it.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await deleteSession(sessionId));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleSaveSidecars() {
    setSidecarBusy(true);
    setComposerError(null);
    try {
      const next = await saveSidecarConfig(sidecarDraft);
      setSidecarState(next);
      setSidecarDraft({
        browserPath: next.browser.path,
        computerPath: next.computer.path,
        autoConfigure: next.autoConfigure
      });
      setComposerError(next.lastError);
    } finally {
      setSidecarBusy(false);
    }
  }

  async function handleAddMcpServer() {
    const name = mcpDraft.name.trim();
    const command = mcpDraft.command.trim();
    if (!name || !command) return;
    const id = `${name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "mcp"}-${Date.now()}`;
    const env =
      mcpDraft.envKey.trim() && mcpDraft.envValue
        ? { [mcpDraft.envKey.trim()]: mcpDraft.envValue }
        : {};
    const server: McpServerConfig = {
      id,
      name,
      enabled: true,
      requireApproval: true,
      timeoutMs: 30000,
      transport: {
        type: "stdio",
        command,
        args: mcpDraft.args.split(/\s+/).filter(Boolean),
        env
      }
    };
    setMcpBusy(true);
    setComposerError(null);
    try {
      setMcpState(await upsertMcpServer(server));
      setMcpDraft({ name: "", command: "", args: "", envKey: "", envValue: "" });
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }

  async function handleRefreshMcpServer(serverId: string) {
    setMcpBusy(true);
    setComposerError(null);
    try {
      const next = await refreshMcpServer(serverId);
      setMcpState(next);
      setComposerError(next.lastError);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }

  async function handleMcpPolicy(
    serverId: string,
    enabled: boolean,
    requireApproval: boolean
  ) {
    setMcpBusy(true);
    try {
      setMcpState(await updateMcpServerPolicy(serverId, enabled, requireApproval));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }

  async function handleRemoveMcpServer(serverId: string) {
    setMcpBusy(true);
    try {
      setMcpState(await removeMcpServer(serverId));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }

  async function handleRefreshSkills() {
    setSkillBusy(true);
    try {
      setSkillState(await refreshSkills());
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setSkillBusy(false);
    }
  }

  async function handleSkillPreference(skillId: string, enabled: boolean, trusted: boolean) {
    setSkillBusy(true);
    try {
      setSkillState(await saveSkillPreference(skillId, enabled, trusted));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setSkillBusy(false);
    }
  }

  async function handleSendPrompt(value: string) {
    const nextPrompt = value.trim();
    const sessionId = activeSession?.id;
    if (!nextPrompt || !sessionId || busySessionIds.has(sessionId)) return;
    const automaticSessionTitle =
      activeSession &&
      isAutoSessionName(activeSession.name)
        ? sessionTitleFromPrompt(nextPrompt)
        : null;
    const automaticSessionId = automaticSessionTitle ? activeSession?.id : null;
    if (automaticSessionTitle && automaticSessionId) {
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === automaticSessionId
                  ? { ...session, name: automaticSessionTitle }
                  : session
              )
            }
          : current
      );
    }
    setStreamAnswer("");
    setComposerError(null);
    markSessionBusy(sessionId, true);
    const submittedAt = Date.now();
    setAgentState((current) => {
      if (!current) return current;
      const contextTokensUsed =
        current.contextTokensUsed + Math.ceil(nextPrompt.length / 4) + 6;
      return {
        ...current,
        sessionName: automaticSessionTitle ?? current.sessionName,
        status: "running",
        canCancel: true,
        canRetry: false,
        transcriptMessages: current.transcriptMessages + 1,
        contextTokensUsed,
        contextRemainingPercent: Math.max(
          0,
          ((current.contextWindowTokens - contextTokensUsed) / current.contextWindowTokens) * 100
        ),
        contextUsageEstimated: true,
        messages: [
          ...current.messages,
          { role: "user", content: nextPrompt, timestampMs: submittedAt }
        ]
      };
    });
    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const next = await runAgentTask(nextPrompt, sessionId);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
        setStreamAnswer("");
      }
      setProjectSessionState(await getProjectSessionState());
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const restored = await getAgentState(sessionId);
        setAgentState(restored);
        updateSessionStatus(sessionId, restored.status);
      }
    } finally {
      markSessionBusy(sessionId, false);
    }
  }

  async function handleCancelAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId) return;
    setComposerError(null);
    try {
      const next = await cancelAgentTask(sessionId);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
      }
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    }
  }

  async function handleRetryAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId || busySessionIds.has(sessionId)) return;
    setStreamAnswer("");
    setComposerError(null);
    markSessionBusy(sessionId, true);
    try {
      const next = await retryAgentTask(sessionId);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
      }
      await refreshAgentTrace(true, sessionId);
    } finally {
      markSessionBusy(sessionId, false);
    }
  }

  async function handleRunTool() {
    if (!selectedTool) return;
    setToolBusy(true);
    setComposerError(null);
    try {
      const next = await runTool(selectedTool, toolInput);
      setPhase5(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setToolBusy(false);
    }
  }

  async function handleResolveToolPermission(
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    setToolBusy(true);
    setComposerError(null);
    try {
      const next = await resolveToolPermission(requestId, decision);
      setPhase5(next);
      setComposerError(next.lastError);
    } finally {
      setToolBusy(false);
    }
  }

  async function handleRunOrchestration() {
    if (!orchestrationPrompt.trim()) return;
    setOrchestrationBusy(true);
    setComposerError(null);
    try {
      const next = await runOrchestration(orchestrationPolicy, orchestrationPrompt);
      setPhase6(next);
      setComposerError(next.lastError);
    } finally {
      setOrchestrationBusy(false);
    }
  }

  async function handleIndexRag() {
    setRagBusy(true);
    setComposerError(null);
    try {
      const next = await indexWorkspaceRag();
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setRagBusy(false);
    }
  }

  async function handleSearchRag() {
    if (!ragQuery.trim()) return;
    setRagBusy(true);
    setComposerError(null);
    try {
      const next = await searchRag(ragQuery, 6);
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setRagBusy(false);
    }
  }

  async function handleAnswerWithRag() {
    if (!ragQuery.trim()) return;
    setRagBusy(true);
    setComposerError(null);
    try {
      const next = await answerWithRag(ragQuery, 6);
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setRagBusy(false);
    }
  }

  async function handleCompactContext() {
    setContextBusy(true);
    setComposerError(null);
    try {
      const next = await compactContext();
      setContextState(next);
      setInspectorTab("context");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setContextBusy(false);
    }
  }

  async function handleRunBrowserTool(toolName: string) {
    const value = browserUrl.trim();
    if (!value) return;
    let input = `url=${value}\noutput_dir=.cindx/browser-captures`;
    if (toolName === "web.search") {
      input = `query=${value}`;
    } else if (toolName === "browser.click") {
      input = `url=${value}\nselector=${browserTarget.trim() || "body"}\noutput_dir=.cindx/browser-actions`;
    } else if (toolName === "browser.type") {
      input = `url=${value}\nselector=${browserTarget.trim() || "body"}\ntext=${browserText}\noutput_dir=.cindx/browser-actions`;
    } else if (toolName === "browser.scroll") {
      input = `url=${value}\ndelta_y=600\noutput_dir=.cindx/browser-actions`;
    }
    setBrowserBusy(true);
    setComposerError(null);
    try {
      const next = await runBrowserTool(toolName, input);
      setPhase8(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } finally {
      setBrowserBusy(false);
    }
  }

  async function handleResolveBrowserPermission(
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    setBrowserBusy(true);
    setComposerError(null);
    try {
      const next = await resolveBrowserPermission(requestId, decision);
      setPhase8(next);
      setComposerError(next.lastError);
    } finally {
      setBrowserBusy(false);
    }
  }

  async function handleResolveAgentPermission(
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    const sessionId = activeSession?.id;
    if (!sessionId || busySessionIds.has(sessionId)) return;
    markSessionBusy(sessionId, true);
    setComposerError(null);
    setAgentState((current) =>
      current
        ? {
            ...current,
            status: "running",
            canCancel: true,
            canRetry: false,
            pendingApprovals: current.pendingApprovals.filter(
              (approval) => approval.requestId !== requestId
            )
          }
        : current
    );
    try {
      const next = await resolveAgentPermission(requestId, decision, sessionId);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
      }
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const restored = await getAgentState(sessionId);
        setAgentState(restored);
        updateSessionStatus(sessionId, restored.status);
      }
    } finally {
      markSessionBusy(sessionId, false);
    }
  }

  async function handleExportAgentTrace() {
    setTraceBusy(true);
    setComposerError(null);
    try {
      const next = await exportAgentTraceJsonl(activeSession?.id);
      setAgentTraceState(next);
      setInspectorTab("details");
      setInspectorOpen(true);
      const latestStep = latestTraceStep(next.turns);
      setSelectedTraceStepId((current) => current ?? latestStep?.id ?? null);
      setComposerError(next.lastError);
    } finally {
      setTraceBusy(false);
    }
  }

  return (
    <main
      className="app-shell"
      data-sidebar-open={sidebarOpen}
      data-inspector-open={inspectorOpen}
      style={{ "--inspector-width": `${inspectorWidth}px` } as CSSProperties}
    >
      <header
        className="window-toolbar"
        onMouseDown={(event) => {
          if (event.button !== 0 || (event.target as Element).closest("button")) return;
          void getCurrentWindow().startDragging();
        }}
      >
        <span className="window-toolbar-panel window-toolbar-panel-left" aria-hidden="true" />
        <span className="window-toolbar-panel window-toolbar-panel-right" aria-hidden="true" />
        {activeView !== "settings" && (
          <div className="window-workspace-header">
            <div className="topbar-title">
              <div>
                <h1>
                  {activeView === "trace"
                    ? "Agent Trace"
                    : activeSession?.name ?? "Session"}
                </h1>
                {activeView === "trace" && (
                  <p title={runtime?.workspaceRoot ?? undefined}>
                    {`Trace: ${agentTraceState?.traceId ?? "loading"} · ${
                      agentTraceState?.sessionName ?? activeSession?.name ?? "session"
                    }`}
                  </p>
                )}
              </div>
            </div>
            <div className="topbar-actions">
              <div
                className="context-usage"
                title={`${agentState?.contextTokensUsed ?? 0} of ${
                  agentState?.contextWindowTokens ?? phase4?.provider.contextWindowTokens ?? 128000
                } context tokens${agentState?.contextUsageEstimated ? " (estimated)" : ""}`}
              >
                <span>
                  {agentState?.contextUsageEstimated ? "~" : ""}
                  {formatTokenCount(agentState?.contextTokensUsed ?? 0)} tokens
                </span>
                <strong>{Math.round(agentState?.contextRemainingPercent ?? 100)}% left</strong>
                <progress
                  max={100}
                  value={agentState?.contextRemainingPercent ?? 100}
                  aria-label="Context window remaining"
                />
              </div>
              <div className={`runtime-pill ${statusText === "Ready" ? "ready" : ""}`}>
                <CheckCircle2 size={16} aria-hidden="true" />
                <span>{statusText}</span>
              </div>
            </div>
          </div>
        )}
        <button
          className="window-pane-toggle sidebar-pane-toggle"
          type="button"
          aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          aria-pressed={sidebarOpen}
          data-open={sidebarOpen}
          title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          onClick={() => setSidebarOpen((open) => !open)}
        >
          <span className="window-pane-toggle-icon" aria-hidden="true">
            <PanelLeftClose className="pane-icon-open" />
            <PanelLeftOpen className="pane-icon-closed" />
          </span>
        </button>
        {activeView !== "settings" && (
          <button
            className="window-pane-toggle inspector-pane-toggle"
            type="button"
            aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}
            aria-pressed={inspectorOpen}
            data-open={inspectorOpen}
            title={inspectorOpen ? "Hide inspector" : "Show inspector"}
            onClick={() => setInspectorOpen((open) => !open)}
          >
            <span className="window-pane-toggle-icon" aria-hidden="true">
              <PanelRightClose className="pane-icon-open" />
              <PanelRightOpen className="pane-icon-closed" />
            </span>
          </button>
        )}
      </header>

      <Sidebar
        activeView={activeView}
        projects={projects}
        sessions={sessions}
        busy={projectSessionBusy}
        searchOpen={sidebarSearchOpen}
        searchQuery={sidebarQuery}
        projectCreateOpen={projectCreateOpen}
        projectName={newProjectName}
        onViewChange={handleWorkspaceViewChange}
        onSearchToggle={() => {
          setSidebarSearchOpen((open) => !open);
          if (sidebarSearchOpen) setSidebarQuery("");
        }}
        onSearchQueryChange={setSidebarQuery}
        onProjectCreateToggle={() => setProjectCreateOpen((open) => !open)}
        onProjectNameChange={setNewProjectName}
        onProjectCreate={() => void handleCreateProject()}
        onSessionCreate={() => void handleCreateSession()}
        onProjectSelect={(projectId) => void handleSelectProject(projectId)}
        onSessionSelect={(sessionId) => void handleSelectSession(sessionId)}
        onSessionRename={(sessionId, name) => void handleRenameSession(sessionId, name)}
        onSessionFork={(sessionId) => void handleForkSession(sessionId)}
        onSessionArchive={(sessionId) => void handleArchiveSession(sessionId)}
        onSessionDelete={(sessionId) => void handleDeleteSession(sessionId)}
      />

      <section className="workspace" aria-label="Agent workspace">
        {activeView === "timeline" ? (
          <>
            <SessionThread
              messages={agentState?.messages ?? []}
              timeline={agentState?.timeline ?? []}
              streamAnswer={streamAnswer}
              status={agentState?.status ?? "idle"}
              selectedId={selectedThreadItem?.id ?? null}
              onSelect={(selection) => {
                setSelectedThreadItem(selection);
                setInspectorTab("details");
                setInspectorOpen(true);
              }}
              onEditMessage={(content) => {
                setActiveComposerDraft(content);
                setComposerFocusRequest((request) => request + 1);
              }}
            />

            <Composer
              value={composerDraft}
              working={agentWorking}
              canStop={agentCanCancel}
              canRetry={agentCanRetry}
              error={composerError}
              focusRequest={composerFocusRequest}
              pendingApproval={agentApprovals[0] ?? null}
              permissionBusy={activeSessionBusy}
              onChange={setActiveComposerDraft}
              onSend={(value) => void handleSendPrompt(value)}
              onCancel={() => void handleCancelAgentTask()}
              onRetry={() => void handleRetryAgentTask()}
              onResolvePermission={(requestId, decision) =>
                void handleResolveAgentPermission(requestId, decision)
              }
            />
          </>
        ) : activeView === "trace" ? (
          <section className="trace-view" aria-label="Agent trace">
            <section className="trace-summary">
              <div>
                <span>Status</span>
                <TraceStatusIcon status={agentTraceState?.status ?? "idle"} />
              </div>
              <div>
                <span>Turns</span>
                <strong>{agentTraceState?.turnCount ?? 0}</strong>
              </div>
              <div>
                <span>Steps</span>
                <strong>{agentTraceState?.stepCount ?? 0}</strong>
              </div>
              <div>
                <span>Tools</span>
                <strong>{agentTraceState?.toolCallCount ?? 0}</strong>
              </div>
              <div>
                <span>Permission waits</span>
                <strong>{agentTraceState?.permissionWaitCount ?? 0}</strong>
              </div>
              <button
                className="secondary-button"
                type="button"
                disabled={traceBusy}
                onClick={handleExportAgentTrace}
              >
                <Save size={17} aria-hidden="true" />
                <span>{traceBusy ? "Exporting" : "Export JSONL"}</span>
              </button>
            </section>

            <section className="trace-turns" aria-label="Trace turns">
              {traceTurns.length === 0 ? (
                <div className="empty-audit">
                  <Clock3 size={17} aria-hidden="true" />
                  <span>No agent trace yet</span>
                </div>
              ) : (
                traceTurns.map((turn) => (
                  <article className="trace-turn" key={`${turn.index}-${turn.startedAtMs}`}>
                    <div className="trace-turn-header">
                      <div>
                        <h2>{turn.label}</h2>
                        <div className="trace-turn-meta">
                          <TraceStatusIcon status={turn.status} />
                          <span>{formatDuration(turn.durationMs)}</span>
                        </div>
                      </div>
                      <em>{turn.steps.length} steps</em>
                    </div>
                    <div className="trace-step-list">
                      {turn.steps.map((step) => (
                        <button
                          className={`trace-step ${
                            selectedTraceStep?.id === step.id ? "selected" : ""
                          }`}
                          type="button"
                          key={step.id}
                          onClick={() => {
                            setSelectedTraceStepId(step.id);
                            setInspectorTab("details");
                            setInspectorOpen(true);
                          }}
                        >
                          <span className="trace-step-icon">
                            <TraceIcon step={step} />
                          </span>
                          <span className="trace-step-body">
                            <strong>{step.label}</strong>
                            <small>{step.detail}</small>
                          </span>
                          <span className="trace-step-meta">
                            <TraceStatusIcon status={step.status} />
                            <small>{formatDuration(step.latencyMs)}</small>
                          </span>
                        </button>
                      ))}
                    </div>
                  </article>
                ))
              )}
            </section>
          </section>
        ) : (
          <section
            className="settings-view"
            aria-label="Settings"
            data-active-group={settingsCategory ?? "index"}
            key={settingsCategory ?? "index"}
          >
            {activeSettingsCategory && (
              <nav className="settings-page-navigation" aria-label="Settings navigation">
                <button
                  className="icon-button settings-back"
                  type="button"
                  aria-label="Back to Settings"
                  title="Back to Settings"
                  onClick={() => setSettingsCategory(null)}
                >
                  <ArrowLeft aria-hidden="true" />
                </button>
              </nav>
            )}
            {settingsCategory === null && (
              <nav className="settings-index" aria-label="Settings categories">
                {settingsCategories.map((category) => (
                  <button
                    type="button"
                    key={category.id}
                    onClick={() => setSettingsCategory(category.id)}
                  >
                    <span className="settings-index-icon">
                      <SettingsCategoryIcon category={category.id} />
                    </span>
                    <span>
                      <strong>{category.label}</strong>
                      <small>{category.description}</small>
                    </span>
                    <ChevronRight aria-hidden="true" />
                  </button>
                ))}
              </nav>
            )}

            <section className="settings-section" data-settings-group="runtime">
              <div className="section-title">
                <LayoutDashboard size={17} aria-hidden="true" />
                <h2>Workspace</h2>
              </div>
              <div className="provider-form">
                <label>
                  <span>Path</span>
                  <input
                    value={workspaceDraft}
                    onChange={(event) => setWorkspaceDraft(event.target.value)}
                  />
                </label>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={workspaceBusy || !workspaceDraft.trim()}
                  onClick={handleSaveWorkspace}
                >
                  <Save size={17} aria-hidden="true" />
                  <span>{workspaceBusy ? "Saving" : "Save workspace"}</span>
                </button>
              </div>
            </section>

            <section className="settings-section" data-settings-group="runtime">
              <div className="section-title">
                <TerminalSquare size={17} aria-hidden="true" />
                <h2>Sidecars</h2>
              </div>
              <div className="rag-stats">
                <div>
                  <strong>{sidecarState?.browser.healthy ? "Ready" : "Check"}</strong>
                  <span>Browser</span>
                </div>
                <div>
                  <strong>{sidecarState?.computer.healthy ? "Ready" : "Check"}</strong>
                  <span>Computer</span>
                </div>
                <div>
                  <strong>{sidecarState?.autoConfigure ? "Auto" : "Manual"}</strong>
                  <span>Env</span>
                </div>
              </div>
              <div className="provider-form">
                <label>
                  <span>Browser sidecar</span>
                  <input
                    value={sidecarDraft.browserPath}
                    onChange={(event) =>
                      setSidecarDraft({ ...sidecarDraft, browserPath: event.target.value })
                    }
                  />
                </label>
                <div className="tool-schema">
                  <strong>{sidecarState?.browser.envKey ?? "CINDX_BROWSER_SIDECAR"}</strong>
                  <span>{sidecarState?.browser.healthOutput ?? "not checked"}</span>
                </div>
                <label>
                  <span>Computer sidecar</span>
                  <input
                    value={sidecarDraft.computerPath}
                    onChange={(event) =>
                      setSidecarDraft({ ...sidecarDraft, computerPath: event.target.value })
                    }
                  />
                </label>
                <div className="tool-schema">
                  <strong>{sidecarState?.computer.envKey ?? "CINDX_COMPUTER_SIDECAR"}</strong>
                  <span>{sidecarState?.computer.healthOutput ?? "not checked"}</span>
                </div>
                <label className="checkbox-row">
                  <input
                    type="checkbox"
                    checked={sidecarDraft.autoConfigure}
                    onChange={(event) =>
                      setSidecarDraft({
                        ...sidecarDraft,
                        autoConfigure: event.target.checked
                      })
                    }
                  />
                  <span>Auto configure tool environment</span>
                </label>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={sidecarBusy}
                  onClick={handleSaveSidecars}
                >
                  <Save size={17} aria-hidden="true" />
                  <span>{sidecarBusy ? "Checking" : "Save sidecars"}</span>
                </button>
              </div>
            </section>

            <section className="settings-section" data-settings-group="knowledge">
              <div className="section-title">
                <Database size={17} aria-hidden="true" />
                <h2>Context</h2>
              </div>
              <div className="rag-stats">
                <div>
                  <strong>{contextCheckpoint?.eventCount ?? 0}</strong>
                  <span>Events</span>
                </div>
                <div>
                  <strong>{contextCheckpoint?.taskCount ?? 0}</strong>
                  <span>Tasks</span>
                </div>
                <div>
                  <strong>{formatTime(contextCheckpoint?.generatedAtMs ?? null)}</strong>
                  <span>Generated</span>
                </div>
              </div>
              <div className="audit-meta">
                <span>{contextCheckpoint?.id ?? "no checkpoint"}</span>
                <span>{contextCheckpoint?.path ?? "live preview"}</span>
              </div>
              {contextCheckpoint?.currentGoal && (
                <pre className="source-preview">{contextCheckpoint.currentGoal}</pre>
              )}
              <button
                className="secondary-button"
                type="button"
                disabled={contextBusy}
                onClick={handleCompactContext}
              >
                <Database size={17} aria-hidden="true" />
                <span>{contextBusy ? "Compacting" : "Compact context"}</span>
              </button>
            </section>

            <section className="settings-section" data-settings-group="sessions">
              <div className="section-title">
                <ArchiveRestore aria-hidden="true" />
                <h2>Archived sessions</h2>
              </div>
              {archivedSessions.length === 0 ? (
                <div className="settings-empty">No archived sessions</div>
              ) : (
                <div className="archived-session-list">
                  {archivedSessions.map((session) => (
                    <div className="archived-session-row" key={session.id}>
                      <span>
                        <strong>{session.name}</strong>
                        <small>
                          {projectSessionState?.projects.find(
                            (project) => project.id === session.projectId
                          )?.name ?? "Project"}
                          {session.archivedAtMs
                            ? ` · ${formatTime(session.archivedAtMs)}`
                            : ""}
                        </small>
                      </span>
                      <button
                        className="secondary-button"
                        type="button"
                        disabled={projectSessionBusy}
                        onClick={() => void handleRestoreSession(session.id)}
                      >
                        <ArchiveRestore aria-hidden="true" />
                        <span>Restore</span>
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </section>

            <section className="settings-section" data-settings-group="models">
              <div className="section-title">
                <KeyRound size={17} aria-hidden="true" />
                <h2>Provider</h2>
              </div>
              {providerDraft && (
                <div className="provider-form">
                  <label>
                    <span>Base URL</span>
                    <input
                      value={providerDraft.baseUrl}
                      onChange={(event) =>
                        setProviderDraft({ ...providerDraft, baseUrl: event.target.value })
                      }
                    />
                  </label>
                  <label>
                    <span>API key</span>
                    <input
                      type="password"
                      value={providerDraft.apiKey}
                      autoComplete="off"
                      spellCheck={false}
                      placeholder={phase4?.provider.apiKeySet ? "Configured key" : "Enter API key"}
                      onChange={(event) =>
                        setProviderDraft({ ...providerDraft, apiKey: event.target.value })
                      }
                    />
                  </label>
                  <div className="model-catalog-row">
                    <button
                      className="secondary-button"
                      type="button"
                      disabled={
                        providerModelsBusy ||
                        !providerDraft.baseUrl.trim() ||
                        (!providerDraft.apiKey.trim() && !phase4?.provider.apiKeySet)
                      }
                      onClick={() => void handleLoadProviderModels()}
                    >
                      <RefreshCw aria-hidden="true" />
                      <span>{providerModelsBusy ? "Loading models" : "Load models"}</span>
                    </button>
                    <span>
                      {providerModels.length > 0
                        ? `${providerModels.length} available`
                        : "Uses the provider model catalog"}
                    </span>
                  </div>
                  {providerModelsError && (
                    <div className="settings-inline-error">{providerModelsError}</div>
                  )}
                  <div className="role-grid provider-meta-grid">
                    <label>
                      <span>Collaboration</span>
                      <select
                        value={providerDraft.collaborationPolicy}
                        onChange={(event) =>
                          setProviderDraft({
                            ...providerDraft,
                            collaborationPolicy: event.target.value
                          })
                        }
                      >
                        <option value="auto_router">Adaptive</option>
                        <option value="plan_execute_review">Quality synthesis</option>
                        <option value="single">Single model</option>
                      </select>
                    </label>
                    <label>
                      <span>Context window</span>
                      <select
                        value={providerDraft.contextWindowTokens}
                        onChange={(event) =>
                          setProviderDraft({
                            ...providerDraft,
                            contextWindowTokens: Number(event.target.value)
                          })
                        }
                      >
                        {[32768, 65536, 128000, 200000, 262144, 1000000].map((tokens) => (
                          <option value={tokens} key={tokens}>
                            {tokens >= 1000000 ? "1M" : `${Math.round(tokens / 1000)}k`}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  <ModelSelect
                    label="Default model"
                    value={providerDraft.model}
                    options={providerModelOptions}
                    onChange={(model) =>
                      setProviderDraft({
                        ...providerDraft,
                        model,
                        plannerModel: model,
                        executorModel: model,
                        reviewerModel: model,
                        summarizerModel: model,
                        embeddingModel: providerDraft.embeddingModel
                      })
                    }
                  />
                  <div className="role-grid">
                    <ModelSelect
                      label="Planner"
                      value={providerDraft.plannerModel}
                      options={providerModelOptions}
                      onChange={(plannerModel) =>
                        setProviderDraft({ ...providerDraft, plannerModel })
                      }
                    />
                    <ModelSelect
                      label="Executor"
                      value={providerDraft.executorModel}
                      options={providerModelOptions}
                      onChange={(executorModel) =>
                        setProviderDraft({ ...providerDraft, executorModel })
                      }
                    />
                    <ModelSelect
                      label="Reviewer"
                      value={providerDraft.reviewerModel}
                      options={providerModelOptions}
                      onChange={(reviewerModel) =>
                        setProviderDraft({ ...providerDraft, reviewerModel })
                      }
                    />
                    <ModelSelect
                      label="Summary"
                      value={providerDraft.summarizerModel}
                      options={providerModelOptions}
                      onChange={(summarizerModel) =>
                        setProviderDraft({ ...providerDraft, summarizerModel })
                      }
                    />
                    <ModelSelect
                      label="Embedding"
                      value={providerDraft.embeddingModel}
                      options={providerModelOptions}
                      onChange={(embeddingModel) =>
                        setProviderDraft({ ...providerDraft, embeddingModel })
                      }
                    />
                  </div>
                  <dl className="settings-facts">
                    <div>
                      <dt>Team</dt>
                      <dd>{collaborationModelCount} unique models across 4 roles</dd>
                    </div>
                  </dl>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={providerBusy}
                    onClick={handleSaveProviderConfig}
                  >
                    <Save size={17} aria-hidden="true" />
                    <span>{providerBusy ? "Saving" : "Save provider"}</span>
                  </button>
                </div>
              )}
            </section>

            <section className="settings-section" data-settings-group="agent">
              <div className="section-title">
                <Bot size={17} aria-hidden="true" />
                <h2>Agent system prompt</h2>
              </div>
              {providerDraft && (
                <div className="provider-form">
                  <label className="system-prompt-field">
                    <span>System prompt</span>
                    <textarea
                      value={providerDraft.agentSystemPrompt}
                      maxLength={32000}
                      rows={12}
                      spellCheck={false}
                      onChange={(event) =>
                        setProviderDraft({
                          ...providerDraft,
                          agentSystemPrompt: event.target.value
                        })
                      }
                    />
                  </label>
                  <div className="system-prompt-meta">
                    <span>{providerDraft.agentSystemPrompt.length.toLocaleString()} / 32,000</span>
                  </div>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={providerBusy || !providerDraft.agentSystemPrompt.trim()}
                    onClick={handleSaveProviderConfig}
                  >
                    <Save size={17} aria-hidden="true" />
                    <span>{providerBusy ? "Saving" : "Save prompt"}</span>
                  </button>
                </div>
              )}
            </section>

            <section className="settings-section" data-settings-group="permissions">
              <div className="section-title">
                <ShieldCheck size={17} aria-hidden="true" />
                <h2>Permissions</h2>
              </div>
              <div className={`permission-callout ${hasPendingPermission ? "pending" : ""}`}>
                <TriangleAlert size={14} aria-hidden="true" />
                <span>
                  {hasPendingPermission
                    ? "A local action is waiting for review."
                    : "Write, execute, network, sensitive, and destructive actions require review."}
                </span>
              </div>
              <button
                className="secondary-button"
                type="button"
                disabled={permissionBusy}
                onClick={handleRequestPermission}
              >
                <ShieldQuestion size={17} aria-hidden="true" />
                <span>Request review</span>
              </button>
              <div className="audit-list" aria-label="Permission audit records">
                {permissionRows.length === 0 ? (
                  <div className="empty-audit">
                    <Clock3 size={17} aria-hidden="true" />
                    <span>No audit records yet</span>
                  </div>
                ) : (
                  permissionRows.map((permission) => (
                    <article className="audit-row" key={permission.id}>
                      <div className="audit-header">
                        <strong>{permission.action}</strong>
                        <em className={permission.status}>{permission.status}</em>
                      </div>
                      <p>{permission.reason}</p>
                      <div className="audit-meta">
                        <span>{permission.risk}</span>
                        <span>{formatTime(permission.requestedAtMs)}</span>
                      </div>
                      {permission.decision ? (
                        <div className="audit-decision">
                          <CheckCircle2 size={15} aria-hidden="true" />
                          <span>{permission.decision}</span>
                        </div>
                      ) : (
                        <div className="audit-buttons">
                          <button
                            type="button"
                            disabled={permissionBusy}
                            onClick={() => handleResolvePermission(permission.id, "allow_once")}
                          >
                            <CheckCircle2 size={15} aria-hidden="true" />
                            <span>Approve once</span>
                          </button>
                          <button
                            type="button"
                            disabled={permissionBusy}
                            onClick={() => handleResolvePermission(permission.id, "deny")}
                          >
                            <XCircle size={15} aria-hidden="true" />
                            <span>Deny</span>
                          </button>
                        </div>
                      )}
                    </article>
                  ))
                )}
              </div>
              {agentApprovals.length > 0 && (
                <div className="audit-list" aria-label="Agent approvals">
                  {agentApprovals.map((approval) => (
                    <article className="audit-row" key={approval.requestId}>
                      <div className="audit-header">
                        <strong>{approval.toolName}</strong>
                        <em className="pending">{approval.risk}</em>
                      </div>
                      <p>{approval.reason}</p>
                      <div className="audit-meta">
                        <span>{approval.scope}</span>
                        <span>{formatTime(approval.requestedAtMs)}</span>
                      </div>
                      <pre className="tool-output">{approval.input}</pre>
                      <div className="audit-buttons">
                        {approval.risk !== "destructive" && (
                          <button
                            type="button"
                            disabled={activeSessionBusy}
                            onClick={() =>
                              handleResolveAgentPermission(
                                approval.requestId,
                                "allow_for_session"
                              )
                            }
                          >
                            <ShieldCheck size={15} aria-hidden="true" />
                            <span>Allow session</span>
                          </button>
                        )}
                        <button
                          type="button"
                          disabled={activeSessionBusy}
                          onClick={() =>
                            handleResolveAgentPermission(approval.requestId, "allow_once")
                          }
                        >
                          <CheckCircle2 size={15} aria-hidden="true" />
                          <span>Approve once</span>
                        </button>
                        <button
                          type="button"
                          disabled={activeSessionBusy}
                          onClick={() => handleResolveAgentPermission(approval.requestId, "deny")}
                        >
                          <XCircle size={15} aria-hidden="true" />
                          <span>Deny</span>
                        </button>
                      </div>
                    </article>
                  ))}
                </div>
              )}
              {externalApprovals.length > 0 && (
                <div className="audit-list" aria-label="Tool and browser approvals">
                  {externalApprovals.map((approval) => (
                    <article
                      className="audit-row"
                      key={`${approval.source}-${approval.requestId}`}
                    >
                      <div className="audit-header">
                        <strong>{approval.toolName}</strong>
                        <em className="pending">{approval.risk}</em>
                      </div>
                      <p>{approval.reason}</p>
                      <div className="audit-meta">
                        <span>{approval.source}</span>
                        <span>{approval.scope}</span>
                        <span>{formatTime(approval.requestedAtMs)}</span>
                      </div>
                      <pre className="tool-output">{approval.input}</pre>
                      <div className="audit-buttons">
                        <button
                          type="button"
                          disabled={approval.source === "tool" ? toolBusy : browserBusy}
                          onClick={() =>
                            approval.source === "tool"
                              ? handleResolveToolPermission(approval.requestId, "allow_once")
                              : handleResolveBrowserPermission(approval.requestId, "allow_once")
                          }
                        >
                          <CheckCircle2 size={15} aria-hidden="true" />
                          <span>Approve once</span>
                        </button>
                        <button
                          type="button"
                          disabled={approval.source === "tool" ? toolBusy : browserBusy}
                          onClick={() =>
                            approval.source === "tool"
                              ? handleResolveToolPermission(approval.requestId, "deny")
                              : handleResolveBrowserPermission(approval.requestId, "deny")
                          }
                        >
                          <XCircle size={15} aria-hidden="true" />
                          <span>Deny</span>
                        </button>
                      </div>
                    </article>
                  ))}
                </div>
              )}
            </section>

            <section className="settings-section" data-settings-group="models">
              <div className="section-title">
                <Play size={17} aria-hidden="true" />
                <h2>Orchestration</h2>
              </div>
              <details className="advanced-settings">
                <summary>
                  <DisclosureTriangle />
                  <span>Manual workflow test</span>
                </summary>
                <div className="tool-runner">
                <label>
                  <span>Policy</span>
                  <select
                    value={orchestrationPolicy}
                    onChange={(event) => setOrchestrationPolicy(event.target.value)}
                  >
                    {(runtime?.orchestrationModes ?? [
                      "single",
                      "plan_execute_review",
                      "best_of_n",
                      "auto_router"
                    ]).map((mode) => (
                      <option key={mode} value={mode}>
                        {mode}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  <span>Prompt</span>
                  <textarea
                    value={orchestrationPrompt}
                    onChange={(event) => setOrchestrationPrompt(event.target.value)}
                    rows={4}
                  />
                </label>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={orchestrationBusy || !orchestrationPrompt.trim()}
                  onClick={handleRunOrchestration}
                >
                  <Play size={17} aria-hidden="true" />
                  <span>{orchestrationBusy ? "Running" : "Run workflow"}</span>
                </button>
                </div>
              </details>
            </section>

            <section className="settings-section" data-settings-group="knowledge">
              <div className="section-title">
                <Database size={17} aria-hidden="true" />
                <h2>Knowledge index</h2>
              </div>
              <div className="rag-stats" aria-label="RAG index stats">
                <div>
                  <strong>{ragStats.filesIndexed}</strong>
                  <span>Files</span>
                </div>
                <div>
                  <strong>{ragStats.chunksIndexed}</strong>
                  <span>Chunks</span>
                </div>
                <div>
                  <strong>{formatTime(ragStats.indexedAtMs)}</strong>
                  <span>Indexed</span>
                </div>
                <div>
                  <strong>Graph + RAG</strong>
                  <span>Retrieval</span>
                </div>
              </div>
              <button
                className="secondary-button"
                type="button"
                disabled={ragBusy}
                onClick={handleIndexRag}
              >
                <Database size={17} aria-hidden="true" />
                <span>{ragBusy ? "Working" : "Index workspace"}</span>
              </button>
              <details className="advanced-settings">
                <summary>
                  <DisclosureTriangle />
                  <span>Test retrieval</span>
                </summary>
                <div className="tool-runner">
                <label>
                  <span>Question</span>
                  <textarea
                    value={ragQuery}
                    onChange={(event) => setRagQuery(event.target.value)}
                    rows={4}
                  />
                </label>
                <div className="button-row">
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={ragBusy || !ragQuery.trim()}
                    onClick={handleSearchRag}
                  >
                    <Search size={17} aria-hidden="true" />
                    <span>Search</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={ragBusy || !ragQuery.trim()}
                    onClick={handleAnswerWithRag}
                  >
                    <Send size={17} aria-hidden="true" />
                    <span>Answer</span>
                  </button>
                </div>
                </div>
              </details>
            </section>

            <section className="settings-section" data-settings-group="tools">
              <div className="section-title">
                <Globe2 size={17} aria-hidden="true" />
                <h2>Browser</h2>
              </div>
              <dl className="settings-facts">
                <div>
                  <dt>Status</dt>
                  <dd>{sidecarState?.browser.healthy ? "Ready" : "Check configuration"}</dd>
                </div>
                <div>
                  <dt>Controller</dt>
                  <dd>Local sidecar</dd>
                </div>
                <div>
                  <dt>Permission</dt>
                  <dd>Reviewed</dd>
                </div>
              </dl>
              <details className="advanced-settings">
                <summary>
                  <DisclosureTriangle />
                  <span>Manual browser controls</span>
                </summary>
                <div className="tool-runner">
                <label>
                  <span>URL or query</span>
                  <input
                    value={browserUrl}
                    onChange={(event) => setBrowserUrl(event.target.value)}
                  />
                </label>
                <label>
                  <span>Target</span>
                  <input
                    value={browserTarget}
                    onChange={(event) => setBrowserTarget(event.target.value)}
                  />
                </label>
                <label>
                  <span>Text</span>
                  <input
                    value={browserText}
                    onChange={(event) => setBrowserText(event.target.value)}
                  />
                </label>
                <div className="button-row">
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("web.search")}
                  >
                    <Search size={17} aria-hidden="true" />
                    <span>Search web</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.open")}
                  >
                    <Globe2 size={17} aria-hidden="true" />
                    <span>Open</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.extract_text")}
                  >
                    <FileText size={17} aria-hidden="true" />
                    <span>Extract</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.capture")}
                  >
                    <Activity size={17} aria-hidden="true" />
                    <span>Capture</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.click")}
                  >
                    <Activity size={17} aria-hidden="true" />
                    <span>Click</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.type")}
                  >
                    <FileText size={17} aria-hidden="true" />
                    <span>Type</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserUrl.trim()}
                    onClick={() => handleRunBrowserTool("browser.scroll")}
                  >
                    <Activity size={17} aria-hidden="true" />
                    <span>Scroll</span>
                  </button>
                </div>
                </div>
              </details>
            </section>

            <section className="settings-section" data-settings-group="tools">
              <div className="section-title">
                <Activity size={17} aria-hidden="true" />
                <h2>Tools</h2>
              </div>
              <dl className="settings-facts">
                <div>
                  <dt>Registered</dt>
                  <dd>{phase5?.tools.length ?? runtime?.registeredTools.length ?? 0}</dd>
                </div>
                <div>
                  <dt>Scope</dt>
                  <dd>Active workspace</dd>
                </div>
                <div>
                  <dt>Execution</dt>
                  <dd>Permission gated</dd>
                </div>
              </dl>
              <details className="advanced-settings">
                <summary>
                  <DisclosureTriangle />
                  <span>Manual tool runner</span>
                </summary>
                <div className="tool-runner">
                <label>
                  <span>Tool</span>
                  <select
                    value={selectedTool}
                    onChange={(event) => {
                      const nextTool = event.target.value;
                      setSelectedTool(nextTool);
                      if (nextTool === "file.read") setToolInput("path=README.md");
                      if (nextTool === "file.list") setToolInput("path=.");
                      if (nextTool === "file.search") setToolInput("path=.\nquery=Phase");
                      if (nextTool === "file.write")
                        setToolInput("path=.cindx/demo.txt\ncontent=hello from Cindx");
                      if (nextTool === "shell.run") setToolInput("command=pwd\ncwd=.");
                      if (nextTool === "web.search") setToolInput("query=local agent");
                      if (nextTool === "browser.open") setToolInput("url=https://example.com");
                      if (nextTool === "browser.extract_text") setToolInput("url=https://example.com");
                      if (nextTool === "browser.capture")
                        setToolInput("url=https://example.com\noutput_dir=.cindx/browser-captures");
                      if (nextTool === "browser.click")
                        setToolInput("url=https://example.com\nselector=body\noutput_dir=.cindx/browser-actions");
                      if (nextTool === "browser.type")
                        setToolInput("url=https://example.com\nselector=body\ntext=hello\noutput_dir=.cindx/browser-actions");
                      if (nextTool === "browser.scroll")
                        setToolInput("url=https://example.com\ndelta_y=600\noutput_dir=.cindx/browser-actions");
                      if (nextTool === "computer.screenshot")
                        setToolInput("redaction=manual\noutput_dir=.cindx/computer-actions");
                      if (nextTool === "computer.click")
                        setToolInput("x=120\ny=240\noutput_dir=.cindx/computer-actions");
                      if (nextTool === "computer.type")
                        setToolInput("text=hello\noutput_dir=.cindx/computer-actions");
                      if (nextTool === "computer.key")
                        setToolInput("key=Cmd+S\ndestructive=false\noutput_dir=.cindx/computer-actions");
                      if (nextTool === "computer.scroll")
                        setToolInput("delta_y=600\noutput_dir=.cindx/computer-actions");
                    }}
                  >
                    {(phase5?.tools ?? []).map((tool) => (
                      <option key={tool.name} value={tool.name}>
                        {tool.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  <span>Input</span>
                  <textarea
                    value={toolInput}
                    onChange={(event) => setToolInput(event.target.value)}
                    rows={4}
                  />
                </label>
                {selectedToolSpec && (
                  <div className="tool-schema">
                    <strong>{selectedToolSpec.risk}</strong>
                    <span>{selectedToolSpec.inputSchema}</span>
                  </div>
                )}
                <button
                  className="secondary-button"
                  type="button"
                  disabled={toolBusy}
                  onClick={handleRunTool}
                >
                  <Play size={17} aria-hidden="true" />
                  <span>{toolBusy ? "Running" : "Run tool"}</span>
                </button>
                </div>
              </details>
            </section>

            <section className="settings-section" data-settings-group="mcp">
              <div className="section-title">
                <Cable size={17} aria-hidden="true" />
                <h2>MCP servers</h2>
              </div>
              <div className="integration-list">
                {(mcpState?.servers ?? []).length === 0 ? (
                  <div className="settings-empty">No MCP servers configured</div>
                ) : (
                  mcpState?.servers.map((server) => (
                    <div className="integration-row" key={server.id}>
                      <div className="integration-main">
                        <strong>{server.name}</strong>
                        <span>
                          {server.transportType === "stdio" ? server.command : server.url}
                        </span>
                        <small>
                          {server.toolCount} tools
                          {server.refreshedAtMs
                            ? ` · refreshed ${formatTime(server.refreshedAtMs)}`
                            : " · catalog not loaded"}
                        </small>
                        {server.lastError && (
                          <small className="settings-inline-error">{server.lastError}</small>
                        )}
                      </div>
                      <div className="integration-controls">
                        <label className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={server.enabled}
                            disabled={mcpBusy}
                            onChange={(event) =>
                              void handleMcpPolicy(
                                server.id,
                                event.target.checked,
                                server.requireApproval
                              )
                            }
                          />
                          <span>Enabled</span>
                        </label>
                        <label className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={server.requireApproval}
                            disabled={mcpBusy}
                            onChange={(event) =>
                              void handleMcpPolicy(
                                server.id,
                                server.enabled,
                                event.target.checked
                              )
                            }
                          />
                          <span>Ask before use</span>
                        </label>
                        <button
                          className="icon-button"
                          type="button"
                          title="Refresh catalog"
                          disabled={mcpBusy || !server.enabled}
                          onClick={() => void handleRefreshMcpServer(server.id)}
                        >
                          <RefreshCw aria-hidden="true" />
                        </button>
                        <button
                          className="icon-button"
                          type="button"
                          title="Remove server"
                          disabled={mcpBusy}
                          onClick={() => void handleRemoveMcpServer(server.id)}
                        >
                          <Trash2 aria-hidden="true" />
                        </button>
                      </div>
                    </div>
                  ))
                )}
              </div>
              <details className="advanced-settings">
                <summary>
                  <DisclosureTriangle />
                  <span>Add stdio server</span>
                </summary>
                <div className="provider-form">
                  <div className="role-grid">
                    <label>
                      <span>Name</span>
                      <input
                        value={mcpDraft.name}
                        onChange={(event) => setMcpDraft({ ...mcpDraft, name: event.target.value })}
                      />
                    </label>
                    <label>
                      <span>Command</span>
                      <input
                        value={mcpDraft.command}
                        spellCheck={false}
                        onChange={(event) =>
                          setMcpDraft({ ...mcpDraft, command: event.target.value })
                        }
                      />
                    </label>
                  </div>
                  <label>
                    <span>Arguments</span>
                    <input
                      value={mcpDraft.args}
                      spellCheck={false}
                      placeholder="--flag value"
                      onChange={(event) => setMcpDraft({ ...mcpDraft, args: event.target.value })}
                    />
                  </label>
                  <div className="role-grid">
                    <label>
                      <span>Secret environment key</span>
                      <input
                        value={mcpDraft.envKey}
                        spellCheck={false}
                        placeholder="API_KEY"
                        onChange={(event) =>
                          setMcpDraft({ ...mcpDraft, envKey: event.target.value })
                        }
                      />
                    </label>
                    <label>
                      <span>Secret value</span>
                      <input
                        type="password"
                        value={mcpDraft.envValue}
                        autoComplete="off"
                        onChange={(event) =>
                          setMcpDraft({ ...mcpDraft, envValue: event.target.value })
                        }
                      />
                    </label>
                  </div>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={mcpBusy || !mcpDraft.name.trim() || !mcpDraft.command.trim()}
                    onClick={() => void handleAddMcpServer()}
                  >
                    <Cable aria-hidden="true" />
                    <span>{mcpBusy ? "Saving" : "Add server"}</span>
                  </button>
                </div>
              </details>
            </section>

            <section className="settings-section" data-settings-group="skills">
              <div className="section-title">
                <BookOpen size={17} aria-hidden="true" />
                <h2>Skills</h2>
              </div>
              <div className="settings-toolbar">
                <span>{skillState?.skills.length ?? 0} discovered</span>
                <button
                  className="icon-button"
                  type="button"
                  title="Refresh skills"
                  disabled={skillBusy}
                  onClick={() => void handleRefreshSkills()}
                >
                  <RefreshCw aria-hidden="true" />
                </button>
              </div>
              <div className="integration-list">
                {(skillState?.skills ?? []).length === 0 ? (
                  <div className="settings-empty">
                    Add SKILL.md packages under .cindx/skills or ~/.cindx/skills
                  </div>
                ) : (
                  skillState?.skills.map((skill) => (
                    <div className="integration-row" key={skill.id}>
                      <div className="integration-main">
                        <strong>{skill.name}</strong>
                        <span>{skill.description || skill.folderName}</span>
                        <small>
                          {skill.scope} · {skill.requiredTools.length} declared tools
                        </small>
                      </div>
                      <div className="integration-controls">
                        <label className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={skill.trusted}
                            disabled={skillBusy}
                            onChange={(event) =>
                              void handleSkillPreference(
                                skill.id,
                                event.target.checked ? skill.enabled : false,
                                event.target.checked
                              )
                            }
                          />
                          <span>Trusted</span>
                        </label>
                        <label className="checkbox-row">
                          <input
                            type="checkbox"
                            checked={skill.enabled}
                            disabled={skillBusy || !skill.trusted}
                            onChange={(event) =>
                              void handleSkillPreference(
                                skill.id,
                                event.target.checked,
                                skill.trusted
                              )
                            }
                          />
                          <span>Enabled</span>
                        </label>
                      </div>
                    </div>
                  ))
                )}
              </div>
            </section>

            <section className="settings-section about-settings" data-settings-group="about">
              <div className="about-app">
                <img src={appIconUrl} alt="" />
                <div>
                  <h2>Cindx</h2>
                  <span>Version {runtime?.appVersion ?? DESKTOP_VERSION}</span>
                </div>
              </div>
              <dl className="settings-facts">
                <div>
                  <dt>Application</dt>
                  <dd>Cindx</dd>
                </div>
                <div>
                  <dt>Version</dt>
                  <dd>{runtime?.appVersion ?? DESKTOP_VERSION}</dd>
                </div>
                <div>
                  <dt>Platform</dt>
                  <dd>Desktop</dd>
                </div>
              </dl>
            </section>
          </section>
        )}
      </section>

      <Inspector
        open={inspectorOpen}
        width={inspectorWidth}
        tab={inspectorTab}
        threadSelection={activeView === "timeline" ? selectedThreadItem : null}
        traceStep={activeView === "trace" ? selectedTraceStep : null}
        traceExportPath={agentTraceState?.exportPath ?? null}
        contextCheckpoint={contextCheckpoint}
        ragAnswer={phase7?.answer ?? null}
        ragSources={ragSources}
        browserObservations={browserObservations}
        toolResults={toolResults}
        traceArtifacts={(agentTraceState?.turns ?? [])
          .flatMap((turn) => turn.steps)
          .filter((step) => Boolean(step.artifactPath))}
        workspaceRoot={runtime?.workspaceRoot ?? ""}
        agentStatus={agentState?.status ?? "idle"}
        agentTurnCount={agentState?.turnCount ?? 0}
        agentMaxTurns={agentState?.maxTurns ?? 24}
        reviewCounts={{
          agent: agentApprovals.length,
          tool: toolApprovals.length,
          browser: browserApprovals.length
        }}
        onTabChange={setInspectorTab}
        onWidthChange={setInspectorWidth}
        onReview={() => {
          if (activeView !== "settings") setWorkspaceViewBeforeSettings(activeView);
          setInspectorOpenBeforeSettings(inspectorOpen);
          setActiveView("settings");
          setSettingsCategory("permissions");
          setInspectorOpen(false);
        }}
      />
    </main>
  );
}
