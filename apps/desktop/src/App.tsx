import {
  Activity,
  ArrowLeft,
  ArchiveRestore,
  BookOpen,
  Bot,
  Bug,
  Cable,
  CheckCircle2,
  Clock3,
  Database,
  FileText,
  Globe2,
  Info,
  KeyRound,
  LayoutDashboard,
  Link2,
  PackagePlus,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Play,
  RefreshCw,
  Save,
  Search,
  Send,
  Settings,
  ShieldCheck,
  ShieldQuestion,
  TerminalSquare,
  Trash2,
  TriangleAlert,
  Workflow,
  XCircle
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent
} from "react";
import { Inspector, type InspectorTab } from "./components/Inspector";
import { Composer } from "./components/Composer";
import { DisclosureTriangle } from "./components/DisclosureTriangle";
import { KnowledgeGraph } from "./components/KnowledgeGraph";
import { Sidebar, type WorkspaceView } from "./components/Sidebar";
import {
  SessionThread,
  type SessionThreadSelection
} from "./components/SessionThread";
import {
  AgentState,
  AgentAttachment,
  AgentEffort,
  AgentTraceState,
  AgentTraceStepView,
  ChatMessageView,
  answerWithRag,
  archiveSession,
  cancelAgentTask,
  compactContext,
  ContextState,
  createProject,
  createSession,
  deleteProject,
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
  getWebSearchConfig,
  generateSessionTitle,
  getMcpState,
  getSkillState,
  installSkillPackage,
  installSkillUrl,
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
  removeAgentAttachment,
  renameProject,
  renameSession,
  restoreSession,
  runAgentTask,
  runBrowserTool,
  runTool,
  runOrchestration,
  saveProviderConfig,
  saveSidecarConfig,
  saveWebSearchConfig,
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
  WebSearchConfigState,
  stageAgentAttachments,
  subscribeToModelStream
} from "./tauri";

const appIconUrl = new URL("../src-tauri/icons/icon.png", import.meta.url).href;
const DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible";

function containsOptimisticUserMessage(
  messages: ChatMessageView[],
  optimistic: ChatMessageView
) {
  return messages.some(
    (message) =>
      message.role === "user" &&
      message.content === optimistic.content &&
      message.timestampMs >= optimistic.timestampMs - 1_000
  );
}

function messagesWithOptimisticUserMessage(
  messages: ChatMessageView[],
  optimistic: ChatMessageView | undefined
) {
  if (!optimistic || containsOptimisticUserMessage(messages, optimistic)) return messages;
  return [...messages, optimistic].sort(
    (left, right) => left.timestampMs - right.timestampMs
  );
}

function loadDebugAlwaysVisible() {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(DEBUG_ALWAYS_VISIBLE_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

function agentEffortFromPolicy(policy: string): AgentEffort {
  if (policy === "single") return "fast";
  if (policy === "best_of_n") return "pro";
  return "auto";
}

function runBudgetForEffort(effort: AgentEffort) {
  if (effort === "fast") return { durationMs: 3 * 60_000, modelCalls: 6, toolCalls: 12 };
  if (effort === "pro") return { durationMs: 60 * 60_000, modelCalls: 96, toolCalls: 180 };
  return { durationMs: 8 * 60_000, modelCalls: 18, toolCalls: 36 };
}

function normalizedEffortPolicy(policy: string) {
  if (policy === "single" || policy === "best_of_n") return policy;
  return "auto_router";
}

function providerDraftFromState(provider: ProviderConfigState): ProviderConfigInput {
  return {
    baseUrl: provider.baseUrl,
    apiKey: "",
    model: provider.model,
    conductorModel: provider.conductorModel,
    plannerModel: provider.plannerModel,
    executorModel: provider.executorModel,
    reviewerModel: provider.reviewerModel,
    summarizerModel: provider.summarizerModel,
    embeddingModel: provider.embeddingModel,
    imageModel: provider.imageModel,
    imageEndpoint: provider.imageEndpoint,
    collaborationPolicy: normalizedEffortPolicy(provider.collaborationPolicy),
    contextWindowTokens: provider.contextWindowTokens,
    agentSystemPrompt: provider.agentSystemPrompt
  };
}

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "local";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(timestampMs);
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

function fileDataBase64(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error(`Failed to read ${file.name}`));
    reader.readAsDataURL(file);
  });
}

function latestTraceStep(turns: { steps: AgentTraceStepView[] }[]) {
  const steps = turns.flatMap((turn) => turn.steps);
  return steps.length > 0 ? steps[steps.length - 1] : null;
}

function agentStateUnchanged(current: AgentState | null, next: AgentState) {
  if (!current) return false;
  const currentMessage = current.messages[current.messages.length - 1];
  const nextMessage = next.messages[next.messages.length - 1];
  const currentTimeline = current.timeline[current.timeline.length - 1];
  const nextTimeline = next.timeline[next.timeline.length - 1];
  const currentApproval = current.pendingApprovals[current.pendingApprovals.length - 1];
  const nextApproval = next.pendingApprovals[next.pendingApprovals.length - 1];

  return (
    current.taskId === next.taskId &&
    current.projectId === next.projectId &&
    current.projectName === next.projectName &&
    current.sessionId === next.sessionId &&
    current.sessionName === next.sessionName &&
    current.status === next.status &&
    current.turnCount === next.turnCount &&
    current.maxTurns === next.maxTurns &&
    current.transcriptMessages === next.transcriptMessages &&
    current.contextTokensUsed === next.contextTokensUsed &&
    current.contextWindowTokens === next.contextWindowTokens &&
    current.contextRemainingPercent === next.contextRemainingPercent &&
    current.contextUsageEstimated === next.contextUsageEstimated &&
    current.runStartedAtMs === next.runStartedAtMs &&
    current.runBudgetMs === next.runBudgetMs &&
    current.runModelCallBudget === next.runModelCallBudget &&
    current.runToolCallBudget === next.runToolCallBudget &&
    current.canCancel === next.canCancel &&
    current.canRetry === next.canRetry &&
    current.canContinue === next.canContinue &&
    current.latestAnswer === next.latestAnswer &&
    current.lastError === next.lastError &&
    current.messages.length === next.messages.length &&
    current.timeline.length === next.timeline.length &&
    current.pendingApprovals.length === next.pendingApprovals.length &&
    currentMessage?.role === nextMessage?.role &&
    currentMessage?.content === nextMessage?.content &&
    currentMessage?.timestampMs === nextMessage?.timestampMs &&
    currentTimeline?.label === nextTimeline?.label &&
    currentTimeline?.detail === nextTimeline?.detail &&
    currentTimeline?.state === nextTimeline?.state &&
    currentTimeline?.timestampMs === nextTimeline?.timestampMs &&
    currentApproval?.requestId === nextApproval?.requestId &&
    currentApproval?.input === nextApproval?.input
  );
}

function agentTraceUnchanged(current: AgentTraceState | null, next: AgentTraceState) {
  if (!current) return false;
  const currentStep = latestTraceStep(current.turns);
  const nextStep = latestTraceStep(next.turns);
  return (
    current.taskId === next.taskId &&
    current.traceId === next.traceId &&
    current.runId === next.runId &&
    current.status === next.status &&
    current.turnCount === next.turnCount &&
    current.stepCount === next.stepCount &&
    current.finishedAtMs === next.finishedAtMs &&
    current.lastError === next.lastError &&
    currentStep?.id === nextStep?.id &&
    currentStep?.status === nextStep?.status &&
    currentStep?.finishedAtMs === nextStep?.finishedAtMs &&
    currentStep?.detail === nextStep?.detail
  );
}

function ModelSelect({
  label,
  value,
  options,
  emptyLabel,
  onChange
}: {
  label: string;
  value: string;
  options: string[];
  emptyLabel?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label>
      <span>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)}>
        {emptyLabel && <option value="">{emptyLabel}</option>}
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
  { id: "runtime", label: "Runtime" },
  { id: "sessions", label: "Sessions" },
  { id: "models", label: "Models" },
  { id: "agent", label: "Agent" },
  { id: "knowledge", label: "Knowledge" },
  { id: "tools", label: "Tools" },
  { id: "mcp", label: "MCP" },
  { id: "skills", label: "Skills" },
  { id: "permissions", label: "Permissions" },
  { id: "about", label: "About" }
] as const;

type SettingsCategory = (typeof settingsCategories)[number]["id"];

function clampSidebarWidth(width: number) {
  return Math.min(320, Math.max(200, width));
}

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
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [activeView, setActiveView] = useState<WorkspaceView>("timeline");
  const [workspaceViewBeforeSettings, setWorkspaceViewBeforeSettings] =
    useState<Exclude<WorkspaceView, "settings">>("timeline");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(236);
  const [sidebarResizing, setSidebarResizing] = useState(false);
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("runtime");
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("details");
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [inspectorOpenBeforeSettings, setInspectorOpenBeforeSettings] = useState(false);
  const [inspectorWidth, setInspectorWidth] = useState(320);
  const [inspectorResizing, setInspectorResizing] = useState(false);
  const [debugAlwaysVisible, setDebugAlwaysVisible] = useState(loadDebugAlwaysVisible);
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
  const [webSearchConfig, setWebSearchConfig] = useState<WebSearchConfigState | null>(null);
  const [mcpState, setMcpState] = useState<McpState | null>(null);
  const [skillState, setSkillState] = useState<SkillState | null>(null);
  const [projectSessionState, setProjectSessionState] = useState<ProjectSessionState | null>(null);
  const [sidecarDraft, setSidecarDraft] = useState({
    browserPath: "",
    computerPath: "",
    autoConfigure: true
  });
  const [webSearchDraft, setWebSearchDraft] = useState({ endpoint: "", apiKey: "" });
  const [providerDraft, setProviderDraft] = useState<ProviderConfigInput | null>(null);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [newProjectName, setNewProjectName] = useState("");
  const [projectCreateOpen, setProjectCreateOpen] = useState(false);
  const [sidebarSearchOpen, setSidebarSearchOpen] = useState(false);
  const [sidebarQuery, setSidebarQuery] = useState("");
  const [agentEffort, setAgentEffort] = useState<AgentEffort>("auto");
  const [composerDrafts, setComposerDrafts] = useState<Record<string, string>>({});
  const [attachmentDrafts, setAttachmentDrafts] = useState<Record<string, AgentAttachment[]>>({});
  const [attachmentBusySessionIds, setAttachmentBusySessionIds] = useState<Set<string>>(
    () => new Set()
  );
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
  const [webSearchBusy, setWebSearchBusy] = useState(false);
  const [mcpBusy, setMcpBusy] = useState(false);
  const [skillBusy, setSkillBusy] = useState(false);
  const [skillRefreshTurn, setSkillRefreshTurn] = useState(0);
  const [skillUrl, setSkillUrl] = useState("");
  const [skillInstallError, setSkillInstallError] = useState<string | null>(null);
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
  const sessionSelectionRequestRef = useRef(0);
  const sessionSelectionQueueRef = useRef<Promise<void>>(Promise.resolve());
  const sessionRefreshRequestRef = useRef(0);
  const trackedSessionTaskIdsRef = useRef<Set<string>>(new Set());
  const optimisticUserMessagesRef = useRef<Map<string, ChatMessageView>>(new Map());
  const skillPackageInputRef = useRef<HTMLInputElement>(null);
  const settingsToastTimerRef = useRef<number | null>(null);
  const [composerError, setComposerError] = useState<string | null>(null);
  const [knowledgeError, setKnowledgeError] = useState<string | null>(null);
  const [webSearchError, setWebSearchError] = useState<string | null>(null);
  const [settingsToast, setSettingsToast] = useState<{ id: number; message: string } | null>(null);

  useEffect(
    () => () => {
      if (settingsToastTimerRef.current !== null) {
        window.clearTimeout(settingsToastTimerRef.current);
      }
    },
    []
  );

  useEffect(() => {
    let disposed = false;
    let deferredLoadTimer: number | null = null;
    let deferredIdleCallback: number | null = null;
    let streamFlushTimer: number | null = null;
    let streamBuffer = "";
    let streamSessionId: string | null = null;
    let unlisten = () => {};

    const flushStreamBuffer = () => {
      if (streamFlushTimer !== null) window.clearTimeout(streamFlushTimer);
      streamFlushTimer = null;
      if (!streamBuffer) return;
      const delta = streamBuffer;
      streamBuffer = "";
      if (streamSessionId && streamSessionId !== activeSessionIdRef.current) return;
      setStreamAnswer((current) => `${current}${delta}`);
    };

    subscribeToModelStream((payload) => {
      if (payload.sessionId && payload.sessionId !== activeSessionIdRef.current) return;
      streamSessionId = payload.sessionId;
      if (payload.reset) {
        streamBuffer = "";
        if (streamFlushTimer !== null) window.clearTimeout(streamFlushTimer);
        streamFlushTimer = null;
        setStreamAnswer("");
      }
      if (payload.done) {
        flushStreamBuffer();
        if (payload.error) setComposerError(payload.error);
        return;
      }
      if (payload.delta) {
        streamBuffer += payload.delta;
        if (streamFlushTimer === null) {
          streamFlushTimer = window.setTimeout(flushStreamBuffer, 40);
        }
      }
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
      getWebSearchConfig().then((state) => {
        setWebSearchConfig(state);
        setWebSearchDraft({ endpoint: state.endpoint, apiKey: "" });
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
        setAgentEffort(agentEffortFromPolicy(state.provider.collaborationPolicy));
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
      if (streamFlushTimer !== null) window.clearTimeout(streamFlushTimer);
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
  const activeSessionBusy = Boolean(activeSession && busySessionIds.has(activeSession.id));

  useEffect(() => {
    activeSessionIdRef.current = activeSession?.id ?? null;
  }, [activeSession?.id]);

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!sessionId || !activeSessionBusy) return;
    let disposed = false;
    let inFlight = false;
    let lastTraceRefreshAt = 0;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const now = Date.now();
        const refreshTrace = now - lastTraceRefreshAt >= 3_000;
        if (refreshTrace) lastTraceRefreshAt = now;
        const [next, nextTrace] = await Promise.all([
          getAgentState(sessionId),
          refreshTrace
            ? getAgentTraceState(sessionId).catch(() => null)
            : Promise.resolve(null)
        ]);
        if (!disposed && activeSessionIdRef.current === sessionId) {
          acknowledgeOptimisticUserMessage(sessionId, next.messages);
          setAgentState((current) => (agentStateUnchanged(current, next) ? current : next));
          if (nextTrace) {
            setAgentTraceState((current) =>
              agentTraceUnchanged(current, nextTrace) ? current : nextTrace
            );
          }
          updateSessionStatus(sessionId, next.status);
        }
      } catch (error) {
        if (!disposed) {
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      } finally {
        inFlight = false;
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), 1000);
    return () => {
      disposed = true;
      window.clearInterval(interval);
    };
  }, [activeSession?.id, activeSessionBusy]);

  const composerDraft = activeSession ? composerDrafts[activeSession.id] ?? "" : "";
  const composerAttachments = activeSession ? attachmentDrafts[activeSession.id] ?? [] : [];
  const attachmentBusy = activeSession
    ? attachmentBusySessionIds.has(activeSession.id)
    : false;

  function setActiveComposerDraft(value: string) {
    const sessionId = activeSessionIdRef.current ?? activeSession?.id;
    if (!sessionId) return;
    setComposerDrafts((current) => ({ ...current, [sessionId]: value }));
  }

  async function handlePickAttachments(files: File[]) {
    const sessionId = activeSessionIdRef.current ?? activeSession?.id;
    if (!sessionId || files.length === 0) return;
    const existing = attachmentDrafts[sessionId] ?? [];
    const available = Math.max(0, 10 - existing.length);
    if (files.length > available) {
      setComposerError("A message can include at most 10 attachments.");
      return;
    }
    setAttachmentBusySessionIds((current) => new Set(current).add(sessionId));
    setComposerError(null);
    try {
      const uploads = await Promise.all(
        files.map(async (file) => ({
          name: file.name,
          mimeType: file.type || "application/octet-stream",
          dataBase64: await fileDataBase64(file)
        }))
      );
      const staged = await stageAgentAttachments(sessionId, uploads);
      setAttachmentDrafts((current) => ({
        ...current,
        [sessionId]: [...(current[sessionId] ?? []), ...staged]
      }));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setAttachmentBusySessionIds((current) => {
        const next = new Set(current);
        next.delete(sessionId);
        return next;
      });
    }
  }

  function handleRemoveAttachment(attachment: AgentAttachment) {
    const sessionId = activeSessionIdRef.current ?? activeSession?.id;
    if (!sessionId) return;
    setAttachmentDrafts((current) => ({
      ...current,
      [sessionId]: (current[sessionId] ?? []).filter((item) => item.id !== attachment.id)
    }));
    void removeAgentAttachment(sessionId, attachment.path).catch((error) => {
      setComposerError(error instanceof Error ? error.message : String(error));
    });
  }

  const normalizedSidebarQuery = sidebarQuery.trim().toLowerCase();

  const sessions = useMemo(
    () =>
      (projectSessionState?.sessions ?? [])
        .filter((session) => {
          if (session.archived) return false;
          if (normalizedSidebarQuery) {
            return `${session.name} ${session.detail}`
              .toLowerCase()
              .includes(normalizedSidebarQuery);
          }
          return !activeProject || session.projectId === activeProject.id;
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
        const projectMatches = `${project.name} ${project.detail} ${project.root}`
          .toLowerCase()
          .includes(normalizedSidebarQuery);
        const matchingSessionExists = (projectSessionState?.sessions ?? []).some(
          (session) =>
            !session.archived &&
            session.projectId === project.id &&
            `${session.name} ${session.detail}`
              .toLowerCase()
              .includes(normalizedSidebarQuery)
        );
        return projectMatches || matchingSessionExists;
      }),
    [normalizedSidebarQuery, projectSessionState?.projects, projectSessionState?.sessions]
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
  const agentCanCancel = Boolean(agentState?.canCancel || activeSessionBusy);
  const agentCanRetry = Boolean(agentState?.canRetry);
  const agentCanContinue = Boolean(agentState?.canContinue);
  const agentWorking = Boolean(activeSessionBusy || agentState?.status === "running");
  const traceTurns = agentTraceState?.turns ?? [];
  const traceSteps = traceTurns.flatMap((turn) => turn.steps);
  const activeSessionTraceSteps =
    activeSession && agentTraceState?.sessionId === activeSession.id ? traceSteps : [];
  const visibleAgentMessages = messagesWithOptimisticUserMessage(
    agentState?.messages ?? [],
    activeSession ? optimisticUserMessagesRef.current.get(activeSession.id) : undefined
  );
  const selectedTraceStep =
    traceSteps.find((step) => step.id === selectedTraceStepId) ?? null;
  const providerModelOptions = useMemo(() => {
    const configured = providerDraft
      ? [
          providerDraft.model,
          providerDraft.conductorModel,
          providerDraft.plannerModel,
          providerDraft.executorModel,
          providerDraft.reviewerModel,
          providerDraft.summarizerModel,
          providerDraft.embeddingModel,
          providerDraft.imageModel
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

  function markSessionTaskStarted(sessionId: string) {
    trackedSessionTaskIdsRef.current.add(sessionId);
    setSessionStatusOverrides((current) => {
      if (!current[sessionId]) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }

  function acknowledgeSessionResult(sessionId: string) {
    setSessionStatusOverrides((current) => {
      if (!["Completed", "Blocked", "Attention"].includes(current[sessionId])) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }

  function updateSessionStatus(sessionId: string, status: AgentState["status"]) {
    const tracked = trackedSessionTaskIdsRef.current.has(sessionId);
    const isTerminal = ["completed", "failed", "cancelled", "idle"].includes(status);
    if (isTerminal) trackedSessionTaskIdsRef.current.delete(sessionId);

    setSessionStatusOverrides((current) => {
      let nextStatus: string | undefined;
      if (tracked && status === "waiting_for_permission") nextStatus = "Review";
      else if (tracked && status === "running") nextStatus = "Working";
      else if (tracked && activeSessionIdRef.current !== sessionId) {
        if (status === "completed") nextStatus = "Completed";
        else if (status === "failed") nextStatus = "Blocked";
        else if (status === "cancelled") nextStatus = "Attention";
      }

      if (nextStatus === undefined) {
        if (!(sessionId in current)) return current;
        const next = { ...current };
        delete next[sessionId];
        return next;
      }
      if (current[sessionId] === nextStatus) return current;
      return { ...current, [sessionId]: nextStatus };
    });
  }

  function acknowledgeOptimisticUserMessage(
    sessionId: string,
    messages: ChatMessageView[]
  ) {
    const optimistic = optimisticUserMessagesRef.current.get(sessionId);
    if (optimistic && containsOptimisticUserMessage(messages, optimistic)) {
      optimisticUserMessagesRef.current.delete(sessionId);
    }
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

  function showSettingsSaved(message = "Saved") {
    if (settingsToastTimerRef.current !== null) {
      window.clearTimeout(settingsToastTimerRef.current);
    }
    setSettingsToast({ id: Date.now(), message });
    settingsToastTimerRef.current = window.setTimeout(() => {
      setSettingsToast(null);
      settingsToastTimerRef.current = null;
    }, 1800);
  }

  async function handleSaveProviderConfig() {
    if (!providerDraft) return;
    setProviderBusy(true);
    setComposerError(null);
    try {
      const next = await saveProviderConfig(providerDraft);
      setPhase4(next);
      setProviderDraft(providerDraftFromState(next.provider));
      setAgentEffort(agentEffortFromPolicy(next.provider.collaborationPolicy));
      showSettingsSaved();
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
      showSettingsSaved();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function refreshWorkspaceAfterProjectSession(nextState: ProjectSessionState) {
    const refreshRequest = ++sessionRefreshRequestRef.current;
    const previousSessionId = activeSessionIdRef.current;
    const sessionId = nextState.activeSessionId || null;
    const nextProject = nextState.projects.find((project) => project.id === nextState.activeProjectId);
    const nextWorkspaceRoot = nextProject?.root ?? "";
    const currentWorkspaceRoot = runtime?.workspaceRoot ?? activeProject?.root ?? "";
    const workspaceChanged = Boolean(
      nextWorkspaceRoot && nextWorkspaceRoot !== currentWorkspaceRoot
    );
    const isCurrentRequest = () =>
      sessionRefreshRequestRef.current === refreshRequest &&
      activeSessionIdRef.current === sessionId;
    const reportBackgroundError = (error: unknown) => {
      if (!isCurrentRequest()) return;
      setComposerError(error instanceof Error ? error.message : String(error));
    };
    const refreshWorkspaceScopedState = () => {
      void getRuntimeStatus()
        .then((nextRuntime) => {
          if (!isCurrentRequest()) return;
          setRuntime(nextRuntime);
          setWorkspaceDraft(nextRuntime.workspaceRoot);
        })
        .catch(reportBackgroundError);
      void getPhase5State()
        .then((next) => {
          if (isCurrentRequest()) setPhase5(next);
        })
        .catch(reportBackgroundError);
      void getPhase7State()
        .then((next) => {
          if (isCurrentRequest()) setPhase7(next);
        })
        .catch(reportBackgroundError);
      void getPhase8State()
        .then((next) => {
          if (isCurrentRequest()) setPhase8(next);
        })
        .catch(reportBackgroundError);
    };

    activeSessionIdRef.current = sessionId;
    if (sessionId) acknowledgeSessionResult(sessionId);
    setProjectSessionState(nextState);
    setComposerError(nextState.lastError);
    setSelectedThreadItem(null);
    setStreamAnswer("");
    if (nextWorkspaceRoot) {
      setWorkspaceDraft(nextWorkspaceRoot);
      setRuntime((current) =>
        current ? { ...current, workspaceRoot: nextWorkspaceRoot } : current
      );
    }
    if (previousSessionId !== sessionId) {
      setAgentState(null);
      setAgentTraceState(null);
      setContextState(null);
      setSelectedTraceStepId(null);
    }
    if (!sessionId) {
      if (workspaceChanged) refreshWorkspaceScopedState();
      return;
    }

    let nextAgentState: AgentState;
    try {
      nextAgentState = await getAgentState(sessionId);
    } catch (error) {
      reportBackgroundError(error);
      return;
    }
    if (!isCurrentRequest()) return;
    acknowledgeOptimisticUserMessage(sessionId, nextAgentState.messages);
    setAgentState(nextAgentState);
    updateSessionStatus(sessionId, nextAgentState.status);

    void getAgentTraceState(sessionId)
      .then((nextTraceState) => {
        if (!isCurrentRequest()) return;
        setAgentTraceState(nextTraceState);
        setSelectedTraceStepId(latestTraceStep(nextTraceState.turns)?.id ?? null);
      })
      .catch(reportBackgroundError);
    void getContextState(sessionId)
      .then((nextContextState) => {
        if (isCurrentRequest()) setContextState(nextContextState);
      })
      .catch(reportBackgroundError);
    if (workspaceChanged) refreshWorkspaceScopedState();
  }

  function enqueueProjectSessionSelection<Result>(operation: () => Promise<Result>) {
    const queued = sessionSelectionQueueRef.current.then(operation, operation);
    sessionSelectionQueueRef.current = queued.then(
      () => undefined,
      () => undefined
    );
    return queued;
  }

  function forgetDeletedSessions(sessionIds: string[]) {
    if (sessionIds.length === 0) return;
    const deleted = new Set(sessionIds);
    const withoutDeletedKeys = <Value,>(current: Record<string, Value>) => {
      const next = { ...current };
      deleted.forEach((sessionId) => delete next[sessionId]);
      return next;
    };
    setComposerDrafts(withoutDeletedKeys);
    setAttachmentDrafts(withoutDeletedKeys);
    setSessionStatusOverrides(withoutDeletedKeys);
    setBusySessionIds((current) => new Set([...current].filter((id) => !deleted.has(id))));
    setAttachmentBusySessionIds(
      (current) => new Set([...current].filter((id) => !deleted.has(id)))
    );
    deleted.forEach((sessionId) => trackedSessionTaskIdsRef.current.delete(sessionId));
  }

  function showWorkspaceView(view: Exclude<WorkspaceView, "settings">) {
    const leavingSettings = activeView === "settings";
    setWorkspaceViewBeforeSettings(view);
    setActiveView(view);
    if (leavingSettings) {
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
    sessionSelectionRequestRef.current += 1;
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await enqueueProjectSessionSelection(() => selectProject(projectId));
      await refreshWorkspaceAfterProjectSession(next);
      showTimelineView();
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleSelectSession(sessionId: string) {
    showTimelineView();
    if (sessionId === activeSessionIdRef.current) return;
    const selectionRequest = ++sessionSelectionRequestRef.current;
    activeSessionIdRef.current = sessionId;
    acknowledgeSessionResult(sessionId);
    setComposerError(null);
    setProjectSessionState((current) => {
      if (!current) return current;
      const target = current.sessions.find((session) => session.id === sessionId);
      if (!target) return current;
      return {
        ...current,
        activeProjectId: target.projectId,
        activeSessionId: sessionId,
        projects: current.projects.map((project) => ({
          ...project,
          active: project.id === target.projectId
        })),
        sessions: current.sessions.map((session) => ({
          ...session,
          active: session.id === sessionId
        }))
      };
    });
    setAgentState(null);
    setAgentTraceState(null);
    setContextState(null);
    setSelectedTraceStepId(null);
    setSelectedThreadItem(null);
    setStreamAnswer("");
    try {
      const next = await enqueueProjectSessionSelection(() => selectSession(sessionId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
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

  async function handleRenameProject(projectId: string, name: string) {
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      await refreshWorkspaceAfterProjectSession(await renameProject(projectId, name));
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProjectSessionBusy(false);
    }
  }

  async function handleDeleteProject(projectId: string) {
    const sessionIds = (projectSessionState?.sessions ?? [])
      .filter((session) => session.projectId === projectId)
      .map((session) => session.id);
    if (
      sessionIds.some(
        (sessionId) =>
          busySessionIds.has(sessionId) || sessionStatusOverrides[sessionId] === "Review"
      )
    ) {
      setComposerError("Stop the running sessions before deleting this project.");
      return;
    }
    setProjectSessionBusy(true);
    setComposerError(null);
    try {
      const next = await deleteProject(projectId);
      forgetDeletedSessions(sessionIds);
      await refreshWorkspaceAfterProjectSession(next);
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
      const next = await deleteSession(sessionId);
      forgetDeletedSessions([sessionId]);
      await refreshWorkspaceAfterProjectSession(next);
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
      showSettingsSaved();
    } finally {
      setSidecarBusy(false);
    }
  }

  async function handleSaveWebSearch() {
    setWebSearchBusy(true);
    setWebSearchError(null);
    setComposerError(null);
    try {
      const next = await saveWebSearchConfig(webSearchDraft);
      setWebSearchConfig(next);
      setWebSearchDraft({ endpoint: next.endpoint, apiKey: "" });
      showSettingsSaved();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setWebSearchError(message);
      setComposerError(message);
    } finally {
      setWebSearchBusy(false);
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
    setSkillRefreshTurn((current) => current + 1);
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

  async function runSkillInstall(request: () => Promise<SkillState>) {
    setSkillBusy(true);
    setSkillInstallError(null);
    try {
      setSkillState(await request());
      showSettingsSaved("Skill installed");
      return true;
    } catch (error) {
      setSkillInstallError(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setSkillBusy(false);
    }
  }

  async function handleInstallSkillPackage(file: File) {
    if (!/\.skill$/i.test(file.name)) {
      setSkillInstallError("Choose a .skill package.");
      return;
    }
    await runSkillInstall(async () => installSkillPackage(await fileDataBase64(file)));
  }

  async function handleInstallSkillUrl() {
    const url = skillUrl.trim();
    if (!url) return;
    if (await runSkillInstall(() => installSkillUrl(url))) setSkillUrl("");
  }

  async function refineAutomaticSessionTitle(sessionId: string, prompt: string) {
    const nextState = await generateSessionTitle(sessionId, prompt);
    const renamedSession = nextState.sessions.find((session) => session.id === sessionId);
    if (!renamedSession) return;
    setProjectSessionState((current) => {
      if (!current) return nextState;
      return {
        ...current,
        sessions: current.sessions.map((session) =>
          session.id === sessionId
            ? {
                ...session,
                name: renamedSession.name,
                updatedAtMs: renamedSession.updatedAtMs
              }
            : session
        )
      };
    });
    setAgentState((current) =>
      current?.sessionId === sessionId
        ? { ...current, sessionName: renamedSession.name }
        : current
    );
  }

  async function handleSendPrompt(value: string) {
    const nextPrompt = value.trim();
    const sessionId = activeSession?.id;
    const attachments = sessionId ? attachmentDrafts[sessionId] ?? [] : [];
    if (
      (!nextPrompt && attachments.length === 0) ||
      !sessionId ||
      busySessionIds.has(sessionId) ||
      attachmentBusySessionIds.has(sessionId)
    ) {
      return;
    }
    const visiblePrompt =
      nextPrompt || `Review attached ${attachments.map((attachment) => attachment.name).join(", ")}`;
    const automaticSessionTitle =
      activeSession &&
      isAutoSessionName(activeSession.name)
        ? sessionTitleFromPrompt(visiblePrompt)
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
    setAttachmentDrafts((current) => ({ ...current, [sessionId]: [] }));
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    const submittedAt = Date.now();
    const optimisticUserMessage: ChatMessageView = {
      role: "user",
      content: visiblePrompt,
      timestampMs: submittedAt
    };
    optimisticUserMessagesRef.current.set(sessionId, optimisticUserMessage);
    const runBudget = runBudgetForEffort(agentEffort);
    setAgentState((current) => {
      if (!current) return current;
      const contextTokensUsed =
        current.contextTokensUsed + Math.ceil(visiblePrompt.length / 4) + attachments.length * 64 + 6;
      return {
        ...current,
        sessionName: automaticSessionTitle ?? current.sessionName,
        status: "running",
        canCancel: true,
        canRetry: false,
        canContinue: false,
        runBudgetMs: runBudget.durationMs,
        runModelCallBudget: runBudget.modelCalls,
        runToolCallBudget: runBudget.toolCalls,
        transcriptMessages: current.transcriptMessages + 1,
        contextTokensUsed,
        contextRemainingPercent: Math.max(
          0,
          ((current.contextWindowTokens - contextTokensUsed) / current.contextWindowTokens) * 100
        ),
        contextUsageEstimated: true,
        runStartedAtMs: submittedAt,
        messages: current.messages
      };
    });
    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      if (automaticSessionId) {
        void refineAutomaticSessionTitle(automaticSessionId, visiblePrompt);
      }
      const next = await runAgentTask(nextPrompt, sessionId, attachments, agentEffort);
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
        setStreamAnswer("");
      }
      setProjectSessionState(await getProjectSessionState());
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      setAttachmentDrafts((current) => ({
        ...current,
        [sessionId]: current[sessionId]?.length ? current[sessionId] : attachments
      }));
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const failedState = await getAgentState(sessionId);
        acknowledgeOptimisticUserMessage(sessionId, failedState.messages);
        setAgentState(failedState);
      }
    } finally {
      markSessionBusy(sessionId, false);
    }
  }

  async function handleCancelAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId) return;
    setStreamAnswer("");
    setComposerError(null);
    try {
      const next = await cancelAgentTask(sessionId);
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status);
      markSessionBusy(sessionId, false);
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
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    try {
      const next = await retryAgentTask(sessionId);
      updateSessionStatus(sessionId, next.status);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState(next);
        setComposerError(next.lastError);
      }
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
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
    setKnowledgeError(null);
    setComposerError(null);
    try {
      const next = await indexWorkspaceRag();
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setKnowledgeError(message);
      setComposerError(message);
    } finally {
      setRagBusy(false);
    }
  }

  async function handleSearchRag() {
    if (!ragQuery.trim()) return;
    setRagBusy(true);
    setKnowledgeError(null);
    setComposerError(null);
    try {
      if (!phase7 || phase7.stats.chunksIndexed === 0) {
        const indexed = await indexWorkspaceRag();
        setPhase7(indexed);
        if (indexed.lastError) {
          setComposerError(indexed.lastError);
          return;
        }
      }
      const next = await searchRag(ragQuery, 6);
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setKnowledgeError(message);
      setComposerError(message);
    } finally {
      setRagBusy(false);
    }
  }

  async function handleAnswerWithRag() {
    if (!ragQuery.trim()) return;
    setRagBusy(true);
    setKnowledgeError(null);
    setComposerError(null);
    try {
      if (!phase7 || phase7.stats.chunksIndexed === 0) {
        const indexed = await indexWorkspaceRag();
        setPhase7(indexed);
        if (indexed.lastError) {
          setComposerError(indexed.lastError);
          return;
        }
      }
      const next = await answerWithRag(ragQuery, 6);
      setPhase7(next);
      setInspectorTab("artifacts");
      setInspectorOpen(true);
      setComposerError(next.lastError);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setKnowledgeError(message);
      setComposerError(message);
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
    if (!value && toolName !== "browser.tabs" && toolName !== "browser.select_tab") return;
    let input = `url=${value}\noutput_dir=.cindx/browser-captures`;
    if (toolName === "web.search") {
      input = `query=${value}`;
    } else if (toolName === "browser.tabs") {
      input = "";
    } else if (toolName === "browser.select_tab") {
      input = `tab_id=${browserTarget.trim()}`;
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
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    setComposerError(null);
    setAgentState((current) =>
      current
        ? {
            ...current,
            status: "running",
            canCancel: true,
            canRetry: false,
            canContinue: false,
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
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        setAgentState(await getAgentState(sessionId));
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
      setInspectorTab("trace");
      setInspectorOpen(true);
      const latestStep = latestTraceStep(next.turns);
      setSelectedTraceStepId((current) => current ?? latestStep?.id ?? null);
      setComposerError(next.lastError);
    } finally {
      setTraceBusy(false);
    }
  }

  function beginSidebarResize(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    setSidebarResizing(true);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const handleMove = (moveEvent: PointerEvent) => {
      setSidebarWidth(clampSidebarWidth(startWidth + moveEvent.clientX - startX));
    };
    const handleUp = () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      setSidebarResizing(false);
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
  }

  return (
    <main
      className="app-shell"
      data-active-view={activeView}
      data-sidebar-open={sidebarOpen}
      data-sidebar-resizing={sidebarResizing}
      data-inspector-open={inspectorOpen}
      data-inspector-resizing={inspectorResizing}
      style={
        {
          "--sidebar-width": `${sidebarWidth}px`,
          "--inspector-width": `${inspectorWidth}px`
        } as CSSProperties
      }
    >
      <header className="window-toolbar" data-tauri-drag-region>
        <span className="window-toolbar-panel window-toolbar-panel-left" aria-hidden="true" />
        <span className="window-toolbar-panel window-toolbar-panel-right" aria-hidden="true" />
        {activeView !== "settings" && (
          <div className="window-workspace-header">
            <div className="topbar-title">
              <div>
                <h1>{activeSession?.name ?? "Session"}</h1>
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

      {sidebarOpen && (
        <div
          className="sidebar-resize-handle"
          role="separator"
          aria-label="Resize sidebar"
          aria-orientation="vertical"
          aria-valuemin={200}
          aria-valuemax={320}
          aria-valuenow={sidebarWidth}
          tabIndex={0}
          onPointerDown={beginSidebarResize}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft") {
              event.preventDefault();
              setSidebarWidth((width) => clampSidebarWidth(width - 16));
            }
            if (event.key === "ArrowRight") {
              event.preventDefault();
              setSidebarWidth((width) => clampSidebarWidth(width + 16));
            }
          }}
        />
      )}

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
        onProjectRename={(projectId, name) => void handleRenameProject(projectId, name)}
        onProjectDelete={(projectId) => void handleDeleteProject(projectId)}
        onSessionSelect={(sessionId) => void handleSelectSession(sessionId)}
        onSessionRename={(sessionId, name) => void handleRenameSession(sessionId, name)}
        onSessionFork={(sessionId) => void handleForkSession(sessionId)}
        onSessionArchive={(sessionId) => void handleArchiveSession(sessionId)}
        onSessionDelete={(sessionId) => void handleDeleteSession(sessionId)}
      />

      <section className="workspace" data-view={activeView} aria-label="Agent workspace">
        {activeView === "timeline" ? (
          <>
            <SessionThread
              sessionId={activeSession?.id ?? null}
              messages={visibleAgentMessages}
              timeline={agentState?.timeline ?? []}
              streamAnswer={streamAnswer}
              status={agentState?.status ?? "idle"}
              runStartedAtMs={agentState?.runStartedAtMs ?? 0}
              selectedId={selectedThreadItem?.id ?? null}
              onSelect={(selection) => {
                setSelectedThreadItem(selection);
                setSelectedTraceStepId(null);
                setInspectorTab("details");
                setInspectorOpen(true);
              }}
              onEditMessage={(content) => {
                setActiveComposerDraft(content);
                setComposerFocusRequest((request) => request + 1);
              }}
              onLinkOpenError={setComposerError}
            />

            <Composer
              value={composerDraft}
              working={agentWorking}
              canStop={agentCanCancel}
              canRetry={agentCanRetry}
              canContinue={agentCanContinue}
              error={composerError}
              focusRequest={composerFocusRequest}
              pendingApproval={agentApprovals[0] ?? null}
              permissionBusy={activeSessionBusy}
              attachments={composerAttachments}
              attachmentBusy={attachmentBusy}
              effort={agentEffort}
              onChange={setActiveComposerDraft}
              onEffortChange={setAgentEffort}
              onSend={(value) => void handleSendPrompt(value)}
              onPickAttachments={(files) => void handlePickAttachments(files)}
              onRemoveAttachment={handleRemoveAttachment}
              onCancel={() => void handleCancelAgentTask()}
              onRetry={() => void handleRetryAgentTask()}
              onDismissError={() => setComposerError(null)}
              onResolvePermission={(requestId, decision) =>
                void handleResolveAgentPermission(requestId, decision)
              }
            />
          </>
        ) : (
          <section
            className="settings-view"
            aria-label="Settings"
            data-active-group={settingsCategory}
          >
            <aside className="settings-sidebar" aria-label="Settings navigation">
              <button
                className="workspace-return-button settings-app-return"
                type="button"
                onClick={showTimelineView}
              >
                <ArrowLeft aria-hidden="true" />
                <span>Back to App</span>
              </button>
              <nav className="settings-tabs" aria-label="Settings categories">
                {settingsCategories.map((category) => (
                  <button
                    className={settingsCategory === category.id ? "active" : ""}
                    type="button"
                    key={category.id}
                    aria-current={settingsCategory === category.id ? "page" : undefined}
                    onClick={() => setSettingsCategory(category.id)}
                  >
                    <SettingsCategoryIcon category={category.id} />
                    <span>{category.label}</span>
                  </button>
                ))}
              </nav>
            </aside>

            <div className="settings-detail">
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
                  <strong
                    className="runtime-state-value"
                    data-state={sidecarState?.browser.healthy ? "ready" : "attention"}
                  >
                    {sidecarState?.browser.healthy ? (
                      <CheckCircle2 aria-hidden="true" />
                    ) : (
                      <TriangleAlert aria-hidden="true" />
                    )}
                    <span>{sidecarState?.browser.healthy ? "Ready" : "Check"}</span>
                  </strong>
                  <span>Browser</span>
                </div>
                <div>
                  <strong
                    className="runtime-state-value"
                    data-state={sidecarState?.computer.healthy ? "ready" : "attention"}
                  >
                    {sidecarState?.computer.healthy ? (
                      <CheckCircle2 aria-hidden="true" />
                    ) : (
                      <TriangleAlert aria-hidden="true" />
                    )}
                    <span>{sidecarState?.computer.healthy ? "Ready" : "Check"}</span>
                  </strong>
                  <span>Computer</span>
                </div>
                <div>
                  <strong
                    className="runtime-state-value"
                    data-state={sidecarState?.autoConfigure ? "auto" : "manual"}
                  >
                    {sidecarState?.autoConfigure ? (
                      <RefreshCw aria-hidden="true" />
                    ) : (
                      <Settings aria-hidden="true" />
                    )}
                    <span>{sidecarState?.autoConfigure ? "Auto" : "Manual"}</span>
                  </strong>
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

            <section className="settings-section" data-settings-group="runtime">
              <div className="section-title">
                <Bug size={17} aria-hidden="true" />
                <h2>Diagnostics</h2>
              </div>
              <label className="checkbox-row">
                <input
                  type="checkbox"
                  checked={debugAlwaysVisible}
                  onChange={(event) => {
                    const visible = event.target.checked;
                    setDebugAlwaysVisible(visible);
                    try {
                      window.localStorage.setItem(
                        DEBUG_ALWAYS_VISIBLE_STORAGE_KEY,
                        String(visible)
                      );
                    } catch {
                      // The preference still applies for this app session.
                    }
                    showSettingsSaved();
                  }}
                />
                <span>Always show Debug</span>
              </label>
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
                      <span>Default effort</span>
                      <select
                        value={providerDraft.collaborationPolicy}
                        onChange={(event) =>
                          setProviderDraft({
                            ...providerDraft,
                            collaborationPolicy: event.target.value
                          })
                        }
                      >
                        <option value="single">Cindx Fast</option>
                        <option value="auto_router">Cindx Auto</option>
                        <option value="best_of_n">Cindx Pro</option>
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
                        conductorModel: model,
                        plannerModel: model,
                        executorModel: model,
                        reviewerModel: model,
                        summarizerModel: model,
                        embeddingModel: providerDraft.embeddingModel
                      })
                    }
                  />
                  <div className="role-grid provider-meta-grid">
                    <ModelSelect
                      label="Conductor"
                      value={providerDraft.conductorModel}
                      options={providerModelOptions}
                      onChange={(conductorModel) =>
                        setProviderDraft({ ...providerDraft, conductorModel })
                      }
                    />
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
                    <ModelSelect
                      label="Image generation"
                      value={providerDraft.imageModel}
                      options={providerModelOptions}
                      emptyLabel="Not configured"
                      onChange={(imageModel) =>
                        setProviderDraft({ ...providerDraft, imageModel })
                      }
                    />
                    <label>
                      <span>Image API endpoint</span>
                      <input
                        value={providerDraft.imageEndpoint}
                        spellCheck={false}
                        placeholder="Uses provider Base URL when empty"
                        onChange={(event) =>
                          setProviderDraft({
                            ...providerDraft,
                            imageEndpoint: event.target.value
                          })
                        }
                      />
                    </label>
                  </div>
                  <dl className="settings-facts">
                    <div>
                      <dt>Team</dt>
                      <dd>{collaborationModelCount} unique models across 4 worker roles</dd>
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
                <h2>Agent instructions</h2>
              </div>
              {providerDraft && (
                <div className="provider-form">
                  <label className="system-prompt-field">
                    <span>Custom instructions</span>
                    <textarea
                      value={providerDraft.agentSystemPrompt}
                      maxLength={32000}
                      rows={10}
                      spellCheck={false}
                      placeholder="Add preferences for tone, workflow, or domain conventions."
                      onChange={(event) =>
                        setProviderDraft({
                          ...providerDraft,
                          agentSystemPrompt: event.target.value
                        })
                      }
                    />
                  </label>
                  <div className="system-prompt-meta">
                    <span>
                      Extends Cindx's protected core behavior; it cannot replace permission or
                      verification rules.
                    </span>
                    <span>{providerDraft.agentSystemPrompt.length.toLocaleString()} / 32,000</span>
                  </div>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={providerBusy}
                    onClick={handleSaveProviderConfig}
                  >
                    <Save size={17} aria-hidden="true" />
                    <span>{providerBusy ? "Saving" : "Save instructions"}</span>
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
                <Workflow size={16} strokeWidth={1.7} aria-hidden="true" />
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
                  <strong>4-way fusion</strong>
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
              <details className="advanced-settings knowledge-graph-details">
                <summary>
                  <DisclosureTriangle />
                  <strong>Graph Explorer</strong>
                  <span>
                    {phase7?.graph.totalNodes ?? 0} nodes · {phase7?.graph.totalEdges ?? 0} edges
                  </span>
                </summary>
                <KnowledgeGraph
                  graph={
                    phase7?.graph ?? {
                      totalNodes: 0,
                      totalEdges: 0,
                      nodes: [],
                      edges: []
                    }
                  }
                />
              </details>
              <div className="tool-runner knowledge-query">
                <label>
                  <span>Question</span>
                  <textarea
                    value={ragQuery}
                    onChange={(event) => setRagQuery(event.target.value)}
                    placeholder="Search the active workspace"
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
              {(knowledgeError || phase7?.lastError) && (
                <div className="settings-inline-error">
                  {knowledgeError || phase7?.lastError}
                </div>
              )}
              {phase7?.retrievalTrace && (
                <details className="advanced-settings retrieval-trace-details">
                  <summary>
                    <DisclosureTriangle />
                    <strong>Retrieval trace</strong>
                    <span>
                      {phase7.retrievalTrace.selectedCount} selected · {phase7.retrievalTrace.durationMs} ms
                    </span>
                  </summary>
                  <div className="retrieval-channel-list">
                    {phase7.retrievalTrace.channels.map((channel) => (
                      <div className="retrieval-channel-row" key={channel.name}>
                        {channel.error ? (
                          <XCircle size={14} aria-label="Failed" />
                        ) : (
                          <CheckCircle2 size={14} aria-label="Complete" />
                        )}
                        <strong>{channel.name.split("_").join(" ")}</strong>
                        <span>{channel.resultCount} hits</span>
                        <time>{channel.durationMs} ms</time>
                        {channel.error && <small>{channel.error}</small>}
                      </div>
                    ))}
                  </div>
                </details>
              )}
              {phase7?.answer && (
                <section className="knowledge-answer" aria-label="Knowledge answer">
                  <strong>Answer</strong>
                  <pre className="source-preview">{phase7.answer}</pre>
                </section>
              )}
              {ragSources.length > 0 && (
                <section className="knowledge-results" aria-label="Knowledge sources">
                  <strong>Sources</strong>
                  {ragSources.slice(0, 6).map((source) => (
                    <article
                      className="knowledge-result"
                      key={`${source.path}-${source.startLine}-${source.fileHash}`}
                    >
                      <header>
                        <strong>{source.path}</strong>
                        <span>{source.score.toFixed(2)}</span>
                      </header>
                      <small>
                        Lines {source.startLine}-{source.endLine} · {source.reason}
                      </small>
                      <p>{source.text}</p>
                    </article>
                  ))}
                </section>
              )}
            </section>

            <section className="settings-section" data-settings-group="tools">
              <div className="section-title">
                <Globe2 size={17} aria-hidden="true" />
                <h2>Web search API</h2>
              </div>
              <div className="provider-form">
                <label>
                  <span>Endpoint</span>
                  <input
                    value={webSearchDraft.endpoint}
                    placeholder="https://search.example.com/api"
                    onChange={(event) =>
                      setWebSearchDraft({ ...webSearchDraft, endpoint: event.target.value })
                    }
                  />
                </label>
                <label>
                  <span>API key</span>
                  <input
                    type="password"
                    value={webSearchDraft.apiKey}
                    autoComplete="off"
                    placeholder={webSearchConfig?.apiKeySet ? "Configured" : "Optional"}
                    onChange={(event) =>
                      setWebSearchDraft({ ...webSearchDraft, apiKey: event.target.value })
                    }
                  />
                </label>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={webSearchBusy}
                  onClick={() => void handleSaveWebSearch()}
                >
                  <Save aria-hidden="true" />
                  <span>{webSearchBusy ? "Saving" : "Save web search"}</span>
                </button>
              </div>
              {webSearchError && (
                <div className="settings-inline-error">{webSearchError}</div>
              )}
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
              <details className="advanced-settings registered-tools-details">
                <summary>
                  <DisclosureTriangle />
                  <span>Registered tools</span>
                  <strong>{phase5?.tools.length ?? runtime?.registeredTools.length ?? 0}</strong>
                </summary>
                <div className="registered-tool-list">
                  {(phase5?.tools ?? []).length > 0
                    ? phase5?.tools.map((tool) => (
                        <div className="registered-tool-row" key={tool.name}>
                          <span>
                            <strong>{tool.name}</strong>
                            <small>{tool.description}</small>
                          </span>
                          <em>{tool.risk}</em>
                        </div>
                      ))
                    : runtime?.registeredTools.map((tool) => (
                        <div className="registered-tool-row" key={tool}>
                          <span>
                            <strong>{tool}</strong>
                          </span>
                        </div>
                      ))}
                </div>
              </details>
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
                  <span>Target or tab ID</span>
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
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy}
                    onClick={() => handleRunBrowserTool("browser.tabs")}
                  >
                    <LayoutDashboard size={17} aria-hidden="true" />
                    <span>List tabs</span>
                  </button>
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={browserBusy || !browserTarget.trim()}
                    onClick={() => handleRunBrowserTool("browser.select_tab")}
                  >
                    <PanelRightOpen size={17} aria-hidden="true" />
                    <span>Select tab</span>
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
                      if (nextTool === "browser.tabs") setToolInput("");
                      if (nextTool === "browser.select_tab")
                        setToolInput("tab_id=<copy from browser.tabs>");
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
              <div className="skill-install">
                <div className="skill-install-copy">
                  <strong>Add skill</strong>
                  <span>Install into this project. New skills stay disabled until trusted.</span>
                </div>
                <input
                  ref={skillPackageInputRef}
                  className="skill-install-input"
                  type="file"
                  accept=".skill,application/zip"
                  onChange={(event) => {
                    const file = event.currentTarget.files?.[0];
                    event.currentTarget.value = "";
                    if (file) void handleInstallSkillPackage(file);
                  }}
                />
                <div className="skill-install-actions">
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={skillBusy}
                    onClick={() => skillPackageInputRef.current?.click()}
                  >
                    <PackagePlus aria-hidden="true" />
                    <span>.skill package</span>
                  </button>
                </div>
                <div className="skill-url-row">
                  <Link2 aria-hidden="true" />
                  <input
                    value={skillUrl}
                    inputMode="url"
                    spellCheck={false}
                    placeholder="https://example.com/my-skill.skill"
                    aria-label="Skill package URL"
                    onChange={(event) => setSkillUrl(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key !== "Enter") return;
                      event.preventDefault();
                      void handleInstallSkillUrl();
                    }}
                  />
                  <button
                    type="button"
                    disabled={skillBusy || !skillUrl.trim()}
                    onClick={() => void handleInstallSkillUrl()}
                  >
                    Add
                  </button>
                </div>
                {skillInstallError && <p className="skill-install-error">{skillInstallError}</p>}
              </div>
              <div className="settings-toolbar">
                <span>{skillState?.skills.length ?? 0} discovered</span>
                <button
                  className="icon-button skills-refresh"
                  type="button"
                  aria-label="Refresh skills"
                  title="Refresh skills"
                  disabled={skillBusy}
                  onClick={() => void handleRefreshSkills()}
                >
                  <RefreshCw
                    aria-hidden="true"
                    className={skillRefreshTurn > 0 ? "skills-refresh-turn" : undefined}
                    key={skillRefreshTurn}
                  />
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
                  <dt>Created by</dt>
                  <dd>Dale, 2026</dd>
                </div>
              </dl>
            </section>
            </div>
          </section>
        )}
      </section>

      <Inspector
        open={inspectorOpen}
        showDebug={debugAlwaysVisible}
        width={inspectorWidth}
        tab={inspectorTab}
        sessionId={activeSession?.id ?? null}
        threadSelection={activeView === "timeline" ? selectedThreadItem : null}
        traceStep={selectedTraceStep}
        traceExportPath={agentTraceState?.exportPath ?? null}
        contextCheckpoint={contextCheckpoint}
        ragAnswer={phase7?.answer ?? null}
        ragSources={ragSources}
        browserObservations={browserObservations}
        toolResults={toolResults}
        sessionTraceSteps={activeSessionTraceSteps}
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
        onTraceStepSelect={(stepId) => {
          setSelectedTraceStepId(stepId);
          setSelectedThreadItem(null);
          setInspectorTab("details");
          setInspectorOpen(true);
        }}
        onTraceExport={() => void handleExportAgentTrace()}
        traceBusy={traceBusy}
        onWidthChange={setInspectorWidth}
        onResizeStart={() => setInspectorResizing(true)}
        onResizeEnd={() => setInspectorResizing(false)}
        onOutputCreated={() => {
          if (activeView === "settings") setInspectorOpenBeforeSettings(true);
          else setInspectorOpen(true);
        }}
        onReview={() => {
          if (activeView !== "settings") setWorkspaceViewBeforeSettings(activeView);
          setInspectorOpenBeforeSettings(inspectorOpen);
          setActiveView("settings");
          setSettingsCategory("permissions");
          setInspectorOpen(false);
        }}
      />
      {settingsToast && (
        <div
          className="settings-saved-toast"
          key={settingsToast.id}
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          <CheckCircle2 aria-hidden="true" />
          <span>{settingsToast.message}</span>
        </div>
      )}
    </main>
  );
}
