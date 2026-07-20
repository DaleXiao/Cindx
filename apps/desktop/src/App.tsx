import {
  Activity,
  ArrowLeft,
  ArchiveRestore,
  BookOpen,
  Bot,
  Bug,
  Cable,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Database,
  Dna,
  EyeOff,
  FileText,
  FolderOpen,
  Globe2,
  Info,
  KeyRound,
  LayoutDashboard,
  Link2,
  Monitor,
  Moon,
  PackagePlus,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  RefreshCw,
  Save,
  Search,
  Send,
  Settings,
  ShieldCheck,
  Sun,
  TerminalSquare,
  Trash2,
  TriangleAlert,
  UserRound,
  Wrench,
  XCircle
} from "lucide-react";
import {
  lazy,
  startTransition,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent
} from "react";
import { Inspector, type InspectorTab } from "./components/Inspector";
import { Composer } from "./components/Composer";
import { QueuedMessages } from "./components/QueuedMessages";
import { Sidebar, type WorkspaceView } from "./components/Sidebar";
import {
  LiveSessionThread,
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
  deleteQueuedAgentMessage,
  deleteProject,
  deleteSession,
  DESKTOP_VERSION,
  exportAgentTraceJsonl,
  getAgentState,
  getAgentStateDelta,
  getAgentHistoryPage,
  getAgentStateRevision,
  getAgentTraceState,
  getContextState,
  getPermissionReviewState,
  getPhase4State,
  getPhase5State,
  getPhase7State,
  getPhase8State,
  getProjectSessionState,
  getRuntimeStatus,
  getSidecarState,
  getWebSearchConfig,
  generateSessionTitle,
  editQueuedAgentMessage,
  getMcpState,
  getPersonalizationConfig,
  getSkillState,
  installSkillPackage,
  installSkillUrl,
  indexWorkspaceRag,
  forkSession,
  listProviderModels,
  validateImageEndpoint,
  Phase4State,
  Phase5State,
  Phase7State,
  Phase8State,
  McpServerConfig,
  McpState,
  ProviderConfigInput,
  ProviderConfigState,
  PermissionReviewItem,
  PermissionReviewState,
  PersonalizationConfig,
  ProjectSessionState,
  QueuedAgentMessage,
  QueuedAgentMessageActionReceipt,
  QueuedAgentMessageReceipt,
  revealArtifact,
  revealMainWindow,
  setSidebarMaterialWidth,
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
  runNextQueuedAgentMessage,
  runBrowserTool,
  runTool,
  pickWorkspaceFolder,
  saveProviderConfig,
  savePersonalizationConfig,
  setPromptEvolutionEnabled,
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
  setSessionEffort,
  steerQueuedAgentMessage,
  SidecarState,
  WebSearchConfigState,
  stageAgentAttachments,
  queueAgentMessage
} from "./tauri";

const appIconUrl = new URL("../src-tauri/icons/icon.png", import.meta.url).href;
const DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible";
const IGNORED_PERMISSION_REVIEWS_STORAGE_KEY = "cindx.permissions.ignored";
const APPEARANCE_STORAGE_KEY = "cindx.appearance";
const SESSION_STATE_CACHE_LIMIT = 24;
const SESSION_AUXILIARY_CACHE_LIMIT = 8;
const FOREGROUND_AGENT_POLL_INTERVAL_MS = 1_000;
const BACKGROUND_AGENT_POLL_INTERVAL_MS = 5_000;

function queuedMessageClientId() {
  const randomId =
    typeof globalThis.crypto !== "undefined" &&
    typeof globalThis.crypto.randomUUID === "function"
      ? globalThis.crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `agent-queue-client-${randomId}`;
}

const KnowledgeGraph = lazy(() =>
  import("./components/KnowledgeGraph").then((module) => ({
    default: module.KnowledgeGraph
  }))
);

const ScheduleView = lazy(() =>
  import("./components/ScheduleView").then((module) => ({
    default: module.ScheduleView
  }))
);

type AppearanceMode = "light" | "dark" | "system";

const DEFAULT_PERSONALIZATION: PersonalizationConfig = {
  preferredName: "",
  responseTone: "natural",
  responseLength: "balanced"
};

function rememberSessionState<Value>(
  cache: Map<string, Value>,
  sessionId: string,
  value: Value,
  limit = SESSION_STATE_CACHE_LIMIT
) {
  cache.delete(sessionId);
  cache.set(sessionId, value);
  while (cache.size > limit) {
    const oldestSessionId = cache.keys().next().value;
    if (!oldestSessionId) break;
    cache.delete(oldestSessionId);
  }
}

function readSessionState<Value>(cache: Map<string, Value>, sessionId: string) {
  const value = cache.get(sessionId);
  if (value === undefined) return null;
  cache.delete(sessionId);
  cache.set(sessionId, value);
  return value;
}

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

function loadIgnoredPermissionReviewIds() {
  if (typeof window === "undefined") return new Set<string>();
  try {
    const values = JSON.parse(
      window.localStorage.getItem(IGNORED_PERMISSION_REVIEWS_STORAGE_KEY) ?? "[]"
    );
    return new Set<string>(Array.isArray(values) ? values.filter((value) => typeof value === "string") : []);
  } catch {
    return new Set<string>();
  }
}

function loadAppearanceMode(): AppearanceMode {
  if (typeof window === "undefined") return "system";
  try {
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    if (stored === "light" || stored === "dark") return stored;
  } catch {
    // Fall back to the system appearance when storage is unavailable.
  }
  return "system";
}

function normalizedSessionEffort(effort: string | undefined): AgentEffort {
  if (effort === "fast" || effort === "pro") return effort;
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
    promptEvolutionEnabled: provider.promptEvolutionEnabled,
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

function formatObservedDuration(durationMs: number) {
  if (durationMs <= 0) return "No data";
  if (durationMs < 60_000) return `${Math.max(1, Math.round(durationMs / 1000))}s`;
  return `${Math.round(durationMs / 60_000)}m`;
}

function promptEvolutionProfileLabel(effort: string) {
  if (effort === "fast") return "Fast";
  if (effort === "pro") return "Pro";
  return "Auto";
}

function promptEvolutionEffortStatus(
  effort: Phase4State["promptEvolution"]["efforts"][number]
) {
  if (effort.evaluationInflight) return "Evaluating";
  if (effort.rolloutStatus === "canary") return `Canary ${effort.canaryPercent}%`;
  if (effort.rolloutStatus === "rolled_back") return "Rolled back";
  if (effort.rolloutStatus === "promoted") return "Promoted";
  if (effort.status === "disabled") return "Off";
  return "Stable";
}

function promptEvolutionProfileStatus(
  profile: Phase4State["promptEvolution"]["profiles"][number]
) {
  if (profile.champion) return "Champion";
  if (profile.next) return "Next";
  if (profile.frontier) return "Frontier";
  if (profile.runs) return "Observed";
  return "Queued";
}

function permissionReviewSourceLabel(source: PermissionReviewItem["source"]) {
  if (source === "agent") return "Agent";
  if (source === "browser") return "Browser";
  if (source === "tool") return "Local tool";
  return "System test";
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

function sessionTitleFromFirstRound(prompt: string, answer: string) {
  const lines = answer
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const candidate = lines.find((line) => /^#{1,6}\s+/.test(line)) ?? lines[0] ?? "";
  const compact = candidate
    .replace(/^#{1,6}\s+/, "")
    .replace(/^(?:title|session title|标题|会话标题)\s*[:：]\s*/i, "")
    .replace(/^[`*_'“”‘’\"\s]+|[`*_'“”‘’\"\s]+$/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const title = [...compact].slice(0, 28).join("").replace(/[.,;:!?。，；：！？]+$/g, "").trim();
  return title || sessionTitleFromPrompt(prompt);
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
    current.eventCount === next.eventCount &&
    current.latestSequence === next.latestSequence &&
    current.oldestSequence === next.oldestSequence &&
    current.hasOlderHistory === next.hasOlderHistory &&
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

function mergeAgentStateDelta(
  current: AgentState | null,
  delta: Awaited<ReturnType<typeof getAgentStateDelta>>
) {
  if (!current || current.sessionId !== delta.state.sessionId) return delta.state;
  return mergeAgentStateSnapshot(current, delta.state);
}

function mergeAgentStateSnapshot(current: AgentState | null, incoming: AgentState) {
  if (!current || current.sessionId !== incoming.sessionId) return incoming;
  const currentHasEarlierHistory =
    current.oldestSequence > 0 &&
    (incoming.oldestSequence === 0 || current.oldestSequence <= incoming.oldestSequence);
  return {
    ...incoming,
    oldestSequence: currentHasEarlierHistory ? current.oldestSequence : incoming.oldestSequence,
    hasOlderHistory: currentHasEarlierHistory
      ? current.hasOlderHistory
      : incoming.hasOlderHistory,
    timeline: mergeSequencedItems(current.timeline, incoming.timeline),
    messages: mergeSequencedItems(current.messages, incoming.messages)
  };
}

function mergeQueuedAgentMessage(
  current: QueuedAgentMessage[],
  incoming: QueuedAgentMessage
) {
  const next = current.filter((message) => message.id !== incoming.id);
  next.push(incoming);
  next.sort((left, right) => {
    if (left.mode !== right.mode) return left.mode === "steer" ? -1 : 1;
    if (left.mode === "steer") {
      return right.updatedAtMs - left.updatedAtMs || left.id.localeCompare(right.id);
    }
    return left.createdAtMs - right.createdAtMs || left.id.localeCompare(right.id);
  });
  return next;
}

function mergeSequencedItems<Item extends { sequence?: number }>(
  current: Item[],
  incoming: Item[]
) {
  if (incoming.length === 0) return current;
  if (current.length === 0) return incoming;
  const currentFirst = current[0]?.sequence;
  const currentLast = current[current.length - 1]?.sequence;
  const incomingFirst = incoming[0]?.sequence;
  const incomingLast = incoming[incoming.length - 1]?.sequence;
  if (
    currentLast !== undefined &&
    incomingFirst !== undefined &&
    currentLast < incomingFirst
  ) {
    return [...current, ...incoming];
  }
  if (
    incomingLast !== undefined &&
    currentFirst !== undefined &&
    incomingLast < currentFirst
  ) {
    return [...incoming, ...current];
  }
  if (
    current.every((item) => item.sequence !== undefined) &&
    incoming.every((item) => item.sequence !== undefined)
  ) {
    const merged: Item[] = [];
    let currentIndex = 0;
    let incomingIndex = 0;
    while (currentIndex < current.length || incomingIndex < incoming.length) {
      const currentItem = current[currentIndex];
      const incomingItem = incoming[incomingIndex];
      if (!incomingItem) {
        merged.push(currentItem);
        currentIndex += 1;
      } else if (!currentItem) {
        merged.push(incomingItem);
        incomingIndex += 1;
      } else if (currentItem.sequence! < incomingItem.sequence!) {
        merged.push(currentItem);
        currentIndex += 1;
      } else if (incomingItem.sequence! < currentItem.sequence!) {
        merged.push(incomingItem);
        incomingIndex += 1;
      } else {
        merged.push(incomingItem);
        currentIndex += 1;
        incomingIndex += 1;
      }
    }
    return merged;
  }
  const merged = new Map<number | string, Item>();
  current.forEach((item, index) => merged.set(item.sequence ?? `current-${index}`, item));
  incoming.forEach((item, index) => merged.set(item.sequence ?? `incoming-${index}`, item));
  return [...merged.values()].sort(
    (left, right) => (left.sequence ?? Number.MAX_SAFE_INTEGER) - (right.sequence ?? Number.MAX_SAFE_INTEGER)
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
    JSON.stringify(current.roleSummaries) === JSON.stringify(next.roleSummaries) &&
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
  { id: "permissions", label: "Pending Reviews" },
  { id: "personalization", label: "Personalization" },
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
  if (category === "tools") return <Wrench aria-hidden="true" />;
  if (category === "mcp") return <Cable aria-hidden="true" />;
  if (category === "skills") return <BookOpen aria-hidden="true" />;
  if (category === "permissions") return <ShieldCheck aria-hidden="true" />;
  if (category === "personalization") return <UserRound aria-hidden="true" />;
  return <Info aria-hidden="true" />;
}

function SettingsChevron({ action = false }: { action?: boolean }) {
  return (
    <span
      className={action ? "settings-action-chevron" : "settings-disclosure-chevron"}
      aria-hidden="true"
    >
      {action ? <ChevronRight /> : <ChevronDown />}
    </span>
  );
}

export function App() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [activeView, setActiveView] = useState<WorkspaceView>("timeline");
  const [selectedScheduleId, setSelectedScheduleId] = useState<string | null>(null);
  const [workspaceViewBeforeSettings, setWorkspaceViewBeforeSettings] =
    useState<Exclude<WorkspaceView, "settings">>("timeline");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(236);
  const [sidebarResizing, setSidebarResizing] = useState(false);
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("runtime");
  const [personalizationDraft, setPersonalizationDraft] =
    useState<PersonalizationConfig>(DEFAULT_PERSONALIZATION);
  const [personalizationBusy, setPersonalizationBusy] = useState(false);
  const [personalizationError, setPersonalizationError] = useState<string | null>(null);
  const [appearanceMode, setAppearanceMode] = useState<AppearanceMode>(loadAppearanceMode);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("details");
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [inspectorOpenBeforeSettings, setInspectorOpenBeforeSettings] = useState(false);
  const [inspectorOpenBeforeSchedule, setInspectorOpenBeforeSchedule] = useState(false);
  const [inspectorWidth, setInspectorWidth] = useState(320);
  const [inspectorResizing, setInspectorResizing] = useState(false);
  const [debugAlwaysVisible, setDebugAlwaysVisible] = useState(loadDebugAlwaysVisible);
  const [permissionReviewState, setPermissionReviewState] =
    useState<PermissionReviewState | null>(null);
  const [ignoredPermissionReviewIds, setIgnoredPermissionReviewIds] = useState(
    loadIgnoredPermissionReviewIds
  );
  const [phase4, setPhase4] = useState<Phase4State | null>(null);
  const [phase5, setPhase5] = useState<Phase5State | null>(null);
  const [phase7, setPhase7] = useState<Phase7State | null>(null);
  const [phase8, setPhase8] = useState<Phase8State | null>(null);
  const [contextState, setContextState] = useState<ContextState | null>(null);
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [sessionLoadingId, setSessionLoadingId] = useState<string | null>(null);
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
  const [composerDrafts, setComposerDrafts] = useState<Record<string, string>>({});
  const [attachmentDrafts, setAttachmentDrafts] = useState<Record<string, AgentAttachment[]>>({});
  const [attachmentBusySessionIds, setAttachmentBusySessionIds] = useState<Set<string>>(
    () => new Set()
  );
  const [composerFocusRequest, setComposerFocusRequest] = useState(0);
  const [streamResetVersion, setStreamResetVersion] = useState(0);
  const [providerModels, setProviderModels] = useState<string[]>([]);
  const [providerModelsBusy, setProviderModelsBusy] = useState(false);
  const [providerModelsRefreshTurn, setProviderModelsRefreshTurn] = useState(0);
  const [providerModelsError, setProviderModelsError] = useState<string | null>(null);
  const [imageEndpointValidation, setImageEndpointValidation] = useState<
    "idle" | "checking" | "valid" | "invalid"
  >("idle");
  const imageEndpointValidationRequestRef = useRef(0);
  const [selectedTool, setSelectedTool] = useState("file.list");
  const [toolInput, setToolInput] = useState("path=.");
  const [ragQuery, setRagQuery] = useState("What is the Cindx MVP scope?");
  const [knowledgeGraphOpen, setKnowledgeGraphOpen] = useState(false);
  const [browserUrl, setBrowserUrl] = useState("https://example.com");
  const [browserTarget, setBrowserTarget] = useState("body");
  const [browserText, setBrowserText] = useState("hello");
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspacePickerBusy, setWorkspacePickerBusy] = useState(false);
  const [providerBusy, setProviderBusy] = useState(false);
  const [toolBusy, setToolBusy] = useState(false);
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
  const [queuedMessageBusyId, setQueuedMessageBusyId] = useState<string | null>(null);
  const [persistingQueuedMessageIds, setPersistingQueuedMessageIds] = useState<Set<string>>(
    () => new Set()
  );
  const [sessionStatusOverrides, setSessionStatusOverrides] = useState<Record<string, string>>({});
  const activeSessionIdRef = useRef<string | null>(null);
  const agentStateRevisionsRef = useRef<
    Map<string, { eventCount: number; latestSequence: number; latestTimestampMs: number }>
  >(new Map());
  const agentStateCacheRef = useRef<Map<string, AgentState>>(new Map());
  const agentTraceCacheRef = useRef<Map<string, AgentTraceState>>(new Map());
  const contextStateCacheRef = useRef<Map<string, ContextState>>(new Map());
  const agentStateRequestsRef = useRef<Map<string, Promise<AgentState>>>(new Map());
  const agentHistoryRequestsRef = useRef<Set<string>>(new Set());
  const [loadingOlderSessionId, setLoadingOlderSessionId] = useState<string | null>(null);
  const sessionSelectionRequestRef = useRef(0);
  const sessionSelectionRunningRef = useRef(false);
  const sessionSelectionPendingRef = useRef<{
    operation: () => Promise<ProjectSessionState>;
    waiters: Array<{
      resolve: (state: ProjectSessionState) => void;
      reject: (error: unknown) => void;
    }>;
  } | null>(null);
  const sessionRefreshRequestRef = useRef(0);
  const trackedSessionTaskIdsRef = useRef<Set<string>>(new Set());
  const optimisticUserMessagesRef = useRef<Map<string, ChatMessageView>>(new Map());
  const [optimisticUserMessageRevision, setOptimisticUserMessageRevision] = useState(0);
  const optimisticQueuedMessagesRef = useRef<Map<string, QueuedAgentMessage>>(new Map());
  const optimisticallyDeletedQueuedMessagesRef = useRef<Map<string, string>>(new Map());
  const steeredQueuedMessageIdsRef = useRef<Set<string>>(new Set());
  const queueDrainingSessionIdsRef = useRef<Set<string>>(new Set());
  const suppressQueueDrainSessionIdsRef = useRef<Set<string>>(new Set());
  const startupWindowRevealRequestedRef = useRef(false);
  const skillPackageInputRef = useRef<HTMLInputElement>(null);
  const settingsToastTimerRef = useRef<number | null>(null);
  const personalizationSaveTimerRef = useRef<number | null>(null);
  const personalizationSaveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const personalizationRevisionRef = useRef(0);
  const personalizationDraftRef = useRef<PersonalizationConfig>(DEFAULT_PERSONALIZATION);
  const [composerError, setComposerError] = useState<string | null>(null);
  const [knowledgeError, setKnowledgeError] = useState<string | null>(null);
  const [webSearchError, setWebSearchError] = useState<string | null>(null);
  const [settingsToast, setSettingsToast] = useState<{ id: number; message: string } | null>(null);

  function requestSessionAgentState(sessionId: string) {
    const existing = agentStateRequestsRef.current.get(sessionId);
    if (existing) return existing;
    const request = getAgentState(sessionId)
      .then((next) => {
        agentStateRevisionsRef.current.set(sessionId, {
          eventCount: next.eventCount,
          latestSequence: next.latestSequence,
          latestTimestampMs: 0
        });
        rememberSessionState(agentStateCacheRef.current, sessionId, next);
        return next;
      })
      .finally(() => {
        if (agentStateRequestsRef.current.get(sessionId) === request) {
          agentStateRequestsRef.current.delete(sessionId);
        }
      });
    agentStateRequestsRef.current.set(sessionId, request);
    return request;
  }

  function applySelectedSessionAgentState(
    sessionId: string,
    selectionRequest: number,
    request: Promise<AgentState>
  ) {
    void request
      .then((nextAgentState) => {
        if (
          selectionRequest !== sessionSelectionRequestRef.current ||
          activeSessionIdRef.current !== sessionId
        ) {
          return;
        }
        setSessionLoadingId(null);
        acknowledgeOptimisticUserMessage(sessionId, nextAgentState.messages);
        startTransition(() => {
          setAgentState((current) => {
            const merged = preserveOptimisticQueuedMessages(
              sessionId,
              mergeAgentStateSnapshot(current, nextAgentState)
            );
            return agentStateUnchanged(current, merged) ? current : merged;
          });
        });
        updateSessionStatus(sessionId, nextAgentState.status, nextAgentState.canContinue);
      })
      .catch((error) => {
        if (
          selectionRequest === sessionSelectionRequestRef.current &&
          activeSessionIdRef.current === sessionId
        ) {
          setSessionLoadingId(null);
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      });
  }

  function restoreCachedSessionState(sessionId: string) {
    const cachedAgentState = readSessionState(agentStateCacheRef.current, sessionId);
    const cachedTraceState = readSessionState(agentTraceCacheRef.current, sessionId);
    const cachedContextState = readSessionState(contextStateCacheRef.current, sessionId);
    setSessionLoadingId(cachedAgentState ? null : sessionId);
    startTransition(() => {
      setAgentState(cachedAgentState);
      setAgentTraceState(cachedTraceState);
      setContextState(cachedContextState);
    });
  }

  const handleThreadSelection = useCallback((selection: SessionThreadSelection) => {
    setSelectedThreadItem(selection);
    setSelectedTraceStepId(null);
    setInspectorTab("details");
    setInspectorOpen(true);
  }, []);

  const handleThreadMessageEdit = useCallback((content: string) => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    setComposerDrafts((current) => ({ ...current, [sessionId]: content }));
    setComposerFocusRequest((request) => request + 1);
  }, []);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      void setSidebarMaterialWidth(
        activeView === "settings" || !sidebarOpen ? 0 : sidebarWidth
      ).catch(() => {});
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeView, sidebarOpen, sidebarWidth]);

  useLayoutEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyAppearance = () => {
      const resolved =
        appearanceMode === "system"
          ? systemTheme.matches
            ? "dark"
            : "light"
          : appearanceMode;
      document.documentElement.dataset.appearance = appearanceMode;
      document.documentElement.dataset.theme = resolved;
      document.documentElement.style.colorScheme = resolved;
    };
    applyAppearance();
    if (appearanceMode !== "system") return;
    systemTheme.addEventListener("change", applyAppearance);
    return () => systemTheme.removeEventListener("change", applyAppearance);
  }, [appearanceMode]);

  useEffect(
    () => () => {
      if (settingsToastTimerRef.current !== null) {
        window.clearTimeout(settingsToastTimerRef.current);
      }
      if (personalizationSaveTimerRef.current !== null) {
        window.clearTimeout(personalizationSaveTimerRef.current);
      }
    },
    []
  );

  useEffect(() => {
    let disposed = false;
    let deferredLoadTimer: number | null = null;
    let deferredIdleCallback: number | null = null;

    const coreRequests = [
      getRuntimeStatus().then((state) => {
        setRuntime(state);
        setWorkspaceDraft(state.workspaceRoot);
      }),
      getProjectSessionState().then((state) => {
        setProjectSessionState(state);
        setSessionLoadingId(
          state.activeSessionId && !agentStateCacheRef.current.has(state.activeSessionId)
            ? state.activeSessionId
            : null
        );
        setComposerError((current) => current ?? state.lastError);
      }),
      getAgentState().then((state) => {
        if (state.sessionId) {
          agentStateRevisionsRef.current.set(state.sessionId, {
            eventCount: state.eventCount,
            latestSequence: state.latestSequence,
            latestTimestampMs: 0
          });
          rememberSessionState(agentStateCacheRef.current, state.sessionId, state);
        }
        setAgentState(state);
        setSessionLoadingId(null);
        if (state.sessionId) {
          updateSessionStatus(state.sessionId, state.status, state.canContinue);
        }
        setComposerError((current) => current ?? state.lastError);
      }),
      getPersonalizationConfig().then((state) => {
        personalizationDraftRef.current = state;
        setPersonalizationDraft(state);
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
      getPermissionReviewState().then(setPermissionReviewState);
      getPhase4State().then((state) => {
        setPhase4(state);
        setProviderDraft(providerDraftFromState(state.provider));
        setComposerError(state.lastError);
      });
      getPhase5State().then((state) => {
        setPhase5(state);
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
    };
  }, []);

  useEffect(() => {
    if (startupWindowRevealRequestedRef.current || !runtime || !projectSessionState) return;
    let disposed = false;
    let fontWaitTimer: number | null = null;
    const reveal = async () => {
      try {
        await Promise.race([
          document.fonts.ready,
          new Promise<void>((resolve) => {
            fontWaitTimer = window.setTimeout(resolve, 120);
          })
        ]);
      } catch {
        // A missing font should not keep the native window hidden.
      }
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
      if (disposed || startupWindowRevealRequestedRef.current) return;
      startupWindowRevealRequestedRef.current = true;
      await revealMainWindow().catch(() => {});
    };
    void reveal();
    return () => {
      disposed = true;
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
    };
  }, [projectSessionState, runtime]);

  useEffect(() => {
    const requestId = imageEndpointValidationRequestRef.current + 1;
    imageEndpointValidationRequestRef.current = requestId;
    const imageEndpoint = providerDraft?.imageEndpoint.trim() ?? "";
    const imageModel = providerDraft?.imageModel.trim() ?? "";
    if (!providerDraft || !imageEndpoint || !imageModel) {
      setImageEndpointValidation("idle");
      return;
    }
    try {
      const parsed = new URL(imageEndpoint);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error("unsupported URL");
    } catch {
      setImageEndpointValidation("invalid");
      return;
    }

    setImageEndpointValidation("checking");
    const timer = window.setTimeout(() => {
      void validateImageEndpoint({
        baseUrl: providerDraft.baseUrl,
        imageModel: providerDraft.imageModel,
        imageEndpoint: providerDraft.imageEndpoint
      }).then((result) => {
        if (imageEndpointValidationRequestRef.current !== requestId) return;
        setImageEndpointValidation(result.valid ? "valid" : "invalid");
      });
    }, 600);
    return () => window.clearTimeout(timer);
  }, [
    providerDraft?.baseUrl,
    providerDraft?.imageEndpoint,
    providerDraft?.imageModel
  ]);

  useEffect(() => {
    if (activeView !== "settings" || settingsCategory !== "permissions") return;
    let disposed = false;
    let inFlight = false;
    const refresh = () => {
      if (disposed || inFlight) return;
      inFlight = true;
      void getPermissionReviewState()
        .then((next) => {
          if (!disposed) setPermissionReviewState(next);
        })
        .finally(() => {
          inFlight = false;
        });
    };
    refresh();
    const timer = window.setInterval(refresh, 2_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeView, settingsCategory]);

  useEffect(() => {
    if (!permissionReviewState) return;
    const pendingIds = new Set(
      permissionReviewState.pending.map((review) => review.requestId)
    );
    setIgnoredPermissionReviewIds((current) => {
      const next = new Set([...current].filter((requestId) => pendingIds.has(requestId)));
      if (next.size === current.size) return current;
      try {
        window.localStorage.setItem(
          IGNORED_PERMISSION_REVIEWS_STORAGE_KEY,
          JSON.stringify([...next])
        );
      } catch {
        // Keep the preference for this app session when storage is unavailable.
      }
      return next;
    });
  }, [permissionReviewState]);

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
  const agentEffort = normalizedSessionEffort(activeSession?.effort);
  const activeAgentState =
    activeSession && agentState?.sessionId === activeSession.id ? agentState : null;
  const activeSessionBusy = Boolean(activeSession && busySessionIds.has(activeSession.id));

  function preserveOptimisticQueuedMessages(sessionId: string, state: AgentState) {
    let queuedMessages = state.queuedMessages;
    optimisticallyDeletedQueuedMessagesRef.current.forEach((targetSessionId, queueId) => {
      if (targetSessionId === sessionId) {
        queuedMessages = queuedMessages.filter((message) => message.id !== queueId);
      }
    });
    optimisticQueuedMessagesRef.current.forEach((message) => {
      if (message.sessionId === sessionId) {
        queuedMessages = mergeQueuedAgentMessage(queuedMessages, message);
      }
    });
    return queuedMessages === state.queuedMessages ? state : { ...state, queuedMessages };
  }

  const loadOlderAgentHistory = useCallback(async () => {
    const sessionId = activeAgentState?.sessionId;
    if (
      !activeAgentState ||
      !sessionId ||
      !activeAgentState.hasOlderHistory ||
      activeAgentState.oldestSequence <= 0 ||
      agentHistoryRequestsRef.current.has(sessionId)
    ) {
      return;
    }
    agentHistoryRequestsRef.current.add(sessionId);
    setLoadingOlderSessionId(sessionId);
    try {
      const page = await getAgentHistoryPage(
        sessionId,
        activeAgentState.oldestSequence
      );
      setAgentState((current) => {
        if (!current || current.sessionId !== page.sessionId) return current;
        return {
          ...current,
          oldestSequence: page.oldestSequence,
          hasOlderHistory: page.hasOlderHistory,
          timeline: mergeSequencedItems(page.timeline, current.timeline),
          messages: mergeSequencedItems(page.messages, current.messages)
        };
      });
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      agentHistoryRequestsRef.current.delete(sessionId);
      setLoadingOlderSessionId((current) => (current === sessionId ? null : current));
    }
  }, [activeAgentState?.hasOlderHistory, activeAgentState?.oldestSequence, activeAgentState?.sessionId]);

  useEffect(() => {
    activeSessionIdRef.current = activeSession?.id ?? null;
  }, [activeSession?.id]);

  const handleAgentStreamDone = useCallback((sessionId: string) => {
    if (activeSessionIdRef.current !== sessionId) return;
    void requestSessionAgentState(sessionId)
      .then((next) => {
        if (activeSessionIdRef.current !== sessionId) return;
        acknowledgeOptimisticUserMessage(sessionId, next.messages);
        setAgentState((current) => {
          const merged = preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, next)
          );
          return agentStateUnchanged(current, merged) ? current : merged;
        });
        updateSessionStatus(sessionId, next.status, next.canContinue);
      })
      .catch((error) => {
        if (activeSessionIdRef.current === sessionId) {
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      });
  }, []);

  useEffect(() => {
    if (!agentState?.sessionId) return;
    rememberSessionState(agentStateCacheRef.current, agentState.sessionId, agentState);
  }, [agentState]);

  useEffect(() => {
    if (!agentTraceState?.sessionId) return;
    rememberSessionState(
      agentTraceCacheRef.current,
      agentTraceState.sessionId,
      agentTraceState,
      SESSION_AUXILIARY_CACHE_LIMIT
    );
  }, [agentTraceState]);

  const sessionPrefetchKey = useMemo(() => {
    if (!projectSessionState?.activeProjectId) return "";
    const sessions = projectSessionState.sessions.filter(
      (session) =>
        session.projectId === projectSessionState.activeProjectId && !session.archived
    );
    const activeIndex = sessions.findIndex(
      (session) => session.id === projectSessionState.activeSessionId
    );
    const nearby = activeIndex >= 0
      ? [sessions[activeIndex - 1], sessions[activeIndex + 1]].filter(
          (session): session is (typeof sessions)[number] => Boolean(session)
        )
      : sessions.slice(0, 2);
    return nearby
      .map((session) => session.id)
      .join("|");
  }, [projectSessionState]);

  useEffect(() => {
    if (!sessionPrefetchKey || activeSessionBusy) return;
    let disposed = false;
    let idleCallback: number | null = null;
    let fallbackTimer: number | null = null;
    const sessionIds = sessionPrefetchKey.split("|");
    const prefetch = () => {
      if (disposed) return;
      void Promise.all(
        sessionIds
          .filter((sessionId) => !agentStateCacheRef.current.has(sessionId))
          .map((sessionId) => requestSessionAgentState(sessionId).catch(() => null))
      );
    };
    if (typeof window.requestIdleCallback === "function") {
      idleCallback = window.requestIdleCallback(prefetch, { timeout: 2_000 });
    } else {
      fallbackTimer = window.setTimeout(prefetch, 1_200);
    }
    return () => {
      disposed = true;
      if (idleCallback !== null) window.cancelIdleCallback(idleCallback);
      if (fallbackTimer !== null) window.clearTimeout(fallbackTimer);
    };
  }, [activeSessionBusy, sessionPrefetchKey]);

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!sessionId || !activeSessionBusy) return;
    let disposed = false;
    let inFlight = false;
    let lastTraceRefreshAt = 0;
    let lastBackgroundRefreshAt = 0;
    const refresh = async () => {
      if (inFlight) return;
      const now = Date.now();
      if (
        document.visibilityState === "hidden" &&
        now - lastBackgroundRefreshAt < BACKGROUND_AGENT_POLL_INTERVAL_MS
      ) {
        return;
      }
      if (document.visibilityState === "hidden") lastBackgroundRefreshAt = now;
      inFlight = true;
      try {
        const refreshTrace =
          activeView === "timeline" && inspectorOpen && now - lastTraceRefreshAt >= 3_000;
        if (refreshTrace) lastTraceRefreshAt = now;
        const revision = await getAgentStateRevision(sessionId);
        const previousRevision = agentStateRevisionsRef.current.get(sessionId);
        const stateChanged =
          !previousRevision ||
          previousRevision.eventCount !== revision.eventCount ||
          previousRevision.latestSequence !== revision.latestSequence;
        const [nextDelta, nextTrace] = await Promise.all([
          stateChanged
            ? getAgentStateDelta(sessionId, previousRevision?.latestSequence ?? 0)
            : Promise.resolve(null),
          refreshTrace
            ? getAgentTraceState(sessionId).catch(() => null)
            : Promise.resolve(null)
        ]);
        if (!disposed && activeSessionIdRef.current === sessionId) {
          agentStateRevisionsRef.current.set(sessionId, revision);
          if (nextDelta) {
            acknowledgeOptimisticUserMessage(sessionId, nextDelta.state.messages);
            setAgentState((current) => {
              const next = preserveOptimisticQueuedMessages(
                sessionId,
                mergeAgentStateDelta(current, nextDelta)
              );
              return agentStateUnchanged(current, next) ? current : next;
            });
            updateSessionStatus(
              sessionId,
              nextDelta.state.status,
              nextDelta.state.canContinue
            );
          }
          if (nextTrace) {
            setAgentTraceState((current) =>
              agentTraceUnchanged(current, nextTrace) ? current : nextTrace
            );
          }
        }
      } catch (error) {
        if (!disposed) {
          setComposerError(error instanceof Error ? error.message : String(error));
        }
      } finally {
        inFlight = false;
      }
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    void refresh();
    document.addEventListener("visibilitychange", handleVisibilityChange);
    const interval = window.setInterval(
      () => void refresh(),
      activeView === "timeline"
        ? FOREGROUND_AGENT_POLL_INTERVAL_MS
        : BACKGROUND_AGENT_POLL_INTERVAL_MS
    );
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      window.clearInterval(interval);
    };
  }, [activeSession?.id, activeSessionBusy, activeView, inspectorOpen]);

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!inspectorOpen || !sessionId || activeSessionBusy) return;
    let disposed = false;
    void Promise.all([getAgentTraceState(sessionId), getContextState(sessionId)])
      .then(([nextTrace, nextContext]) => {
        if (disposed || activeSessionIdRef.current !== sessionId) return;
        rememberSessionState(
          agentTraceCacheRef.current,
          sessionId,
          nextTrace,
          SESSION_AUXILIARY_CACHE_LIMIT
        );
        rememberSessionState(
          contextStateCacheRef.current,
          sessionId,
          nextContext,
          SESSION_AUXILIARY_CACHE_LIMIT
        );
        startTransition(() => {
          setAgentTraceState((current) =>
            agentTraceUnchanged(current, nextTrace) ? current : nextTrace
          );
          setContextState(nextContext);
        });
        setSelectedTraceStepId(latestTraceStep(nextTrace.turns)?.id ?? null);
      })
      .catch((error) => {
        if (!disposed) setComposerError(error instanceof Error ? error.message : String(error));
      });
    return () => {
      disposed = true;
    };
  }, [activeSession?.id, activeSessionBusy, inspectorOpen]);

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

  const toolApprovals = phase5?.pendingApprovals ?? [];
  const browserApprovals = phase8?.pendingApprovals ?? [];
  const agentApprovals = activeAgentState?.pendingApprovals ?? [];
  const permissionReviews = permissionReviewState?.pending ?? [];
  const activePermissionReviews = permissionReviews.filter(
    (review) => !ignoredPermissionReviewIds.has(review.requestId)
  );
  const ignoredPermissionReviews = permissionReviews.filter((review) =>
    ignoredPermissionReviewIds.has(review.requestId)
  );
  const toolResults = phase5?.results ?? [];
  const ragStats = phase7?.stats ?? { filesIndexed: 0, chunksIndexed: 0, indexedAtMs: 0 };
  const ragSources = phase7?.sources ?? [];
  const browserObservations = phase8?.observations ?? [];
  const contextCheckpoint = contextState?.checkpoint ?? null;
  const agentCanCancel = Boolean(activeAgentState?.canCancel || activeSessionBusy);
  const agentCanRetry = Boolean(activeAgentState?.canRetry);
  const agentCanContinue = Boolean(activeAgentState?.canContinue);
  const agentWorking = Boolean(activeSessionBusy || activeAgentState?.status === "running");
  const traceTurns = useMemo(
    () => (activeView === "timeline" ? agentTraceState?.turns ?? [] : []),
    [activeView, agentTraceState?.turns]
  );
  const traceSteps = useMemo(
    () => traceTurns.flatMap((turn) => turn.steps),
    [traceTurns]
  );
  const activeSessionTraceSteps = useMemo(
    () =>
      activeSession && agentTraceState?.sessionId === activeSession.id ? traceSteps : [],
    [activeSession, agentTraceState?.sessionId, traceSteps]
  );
  const visibleAgentMessages = useMemo(
    () =>
      activeView === "timeline"
        ? messagesWithOptimisticUserMessage(
            activeAgentState?.messages ?? [],
            activeSession
              ? optimisticUserMessagesRef.current.get(activeSession.id)
              : undefined
          )
        : [],
    [
      activeAgentState?.messages,
      activeSession,
      activeView,
      optimisticUserMessageRevision
    ]
  );
  const selectedTraceStep = useMemo(
    () => traceSteps.find((step) => step.id === selectedTraceStepId) ?? null,
    [selectedTraceStepId, traceSteps]
  );
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
      if (current[sessionId] !== "Completed") {
        return current;
      }
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }

  function clearSessionTransientStatus(sessionId: string) {
    trackedSessionTaskIdsRef.current.delete(sessionId);
    setSessionStatusOverrides((current) => {
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }

  function updateSessionStatus(
    sessionId: string,
    status: AgentState["status"],
    canContinue = false
  ) {
    const tracked = trackedSessionTaskIdsRef.current.has(sessionId);
    const isTerminal = ["paused", "completed", "failed", "cancelled", "idle"].includes(status);
    if (isTerminal) trackedSessionTaskIdsRef.current.delete(sessionId);

    setSessionStatusOverrides((current) => {
      let nextStatus: string | undefined;
      if (status === "waiting_for_permission") nextStatus = "Approval required";
      else if (status === "running") nextStatus = "Working";
      else if (status === "paused" || (status === "completed" && canContinue)) {
        nextStatus = "Paused";
      } else if (status === "failed") nextStatus = "Error";
      else if (status === "cancelled") nextStatus = "Interrupted";
      else if (tracked && status === "completed") nextStatus = "Completed";

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
      setOptimisticUserMessageRevision((revision) => revision + 1);
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

  async function refreshPermissionReviews() {
    const next = await getPermissionReviewState();
    setPermissionReviewState(next);
    return next;
  }

  function persistIgnoredPermissionReviewIds(ids: Set<string>) {
    setIgnoredPermissionReviewIds(ids);
    try {
      window.localStorage.setItem(
        IGNORED_PERMISSION_REVIEWS_STORAGE_KEY,
        JSON.stringify([...ids])
      );
    } catch {
      // Keep the preference for this app session when storage is unavailable.
    }
  }

  function handleIgnorePermissionReview(requestId: string) {
    const next = new Set(ignoredPermissionReviewIds);
    next.add(requestId);
    persistIgnoredPermissionReviewIds(next);
  }

  function handleRestorePermissionReview(requestId: string) {
    const next = new Set(ignoredPermissionReviewIds);
    next.delete(requestId);
    persistIgnoredPermissionReviewIds(next);
  }

  async function handleResolvePermissionReview(
    review: PermissionReviewItem,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    setPermissionBusy(true);
    try {
      if (review.source === "agent") {
        const sessionId = review.sessionId ?? activeSession?.id;
        if (!sessionId) throw new Error("The related session is no longer available.");
        await handleResolveAgentPermission(review.requestId, decision, sessionId);
      } else if (review.source === "tool") {
        await handleResolveToolPermission(review.requestId, decision);
      } else if (review.source === "browser") {
        await handleResolveBrowserPermission(review.requestId, decision);
      } else {
        await resolvePermission(review.requestId, decision);
      }
      handleRestorePermissionReview(review.requestId);
    } finally {
      await refreshPermissionReviews().catch(() => {});
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

  function handleAppearanceModeChange(mode: AppearanceMode) {
    setAppearanceMode(mode);
    try {
      window.localStorage.setItem(APPEARANCE_STORAGE_KEY, mode);
    } catch {
      // Keep the preference for this app session when storage is unavailable.
    }
    showSettingsSaved("Appearance updated");
  }

  function persistPersonalization(
    next: PersonalizationConfig,
    revision: number,
    notify: boolean
  ) {
    setPersonalizationBusy(true);
    setPersonalizationError(null);
    const save = personalizationSaveQueueRef.current
      .catch(() => undefined)
      .then(async () => {
        const saved = await savePersonalizationConfig(next);
        if (revision !== personalizationRevisionRef.current) return;
        personalizationDraftRef.current = saved;
        setPersonalizationDraft(saved);
        if (notify) showSettingsSaved("Personalization saved");
      })
      .catch((error) => {
        if (revision !== personalizationRevisionRef.current) return;
        setPersonalizationError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (revision === personalizationRevisionRef.current) {
          setPersonalizationBusy(false);
        }
      });
    personalizationSaveQueueRef.current = save;
    return save;
  }

  function updatePersonalizationDraft(next: PersonalizationConfig) {
    personalizationDraftRef.current = next;
    setPersonalizationDraft(next);
    setPersonalizationError(null);
    const revision = personalizationRevisionRef.current + 1;
    personalizationRevisionRef.current = revision;
    if (personalizationSaveTimerRef.current !== null) {
      window.clearTimeout(personalizationSaveTimerRef.current);
    }
    personalizationSaveTimerRef.current = window.setTimeout(() => {
      personalizationSaveTimerRef.current = null;
      void persistPersonalization(next, revision, false);
    }, 220);
  }

  function flushPersonalization(notify: boolean) {
    if (personalizationSaveTimerRef.current !== null) {
      window.clearTimeout(personalizationSaveTimerRef.current);
      personalizationSaveTimerRef.current = null;
    }
    const revision = personalizationRevisionRef.current + 1;
    personalizationRevisionRef.current = revision;
    return persistPersonalization(personalizationDraftRef.current, revision, notify);
  }

  async function handleSavePersonalization() {
    await flushPersonalization(true);
  }

  async function handleSaveProviderConfig() {
    if (!providerDraft) return;
    setProviderBusy(true);
    setComposerError(null);
    try {
      const next = await saveProviderConfig(providerDraft);
      setPhase4(next);
      setProviderDraft(providerDraftFromState(next.provider));
      showSettingsSaved();
    } finally {
      setProviderBusy(false);
    }
  }

  async function handlePromptEvolutionToggle(enabled: boolean) {
    if (!providerDraft || providerBusy) return;
    const previous = providerDraft.promptEvolutionEnabled;
    setProviderDraft({ ...providerDraft, promptEvolutionEnabled: enabled });
    setProviderBusy(true);
    try {
      const next = await setPromptEvolutionEnabled(enabled);
      setPhase4(next);
      setProviderDraft(providerDraftFromState(next.provider));
      showSettingsSaved(enabled ? "Prompt evolution enabled" : "Prompt evolution disabled");
    } catch (error) {
      setProviderDraft({ ...providerDraft, promptEvolutionEnabled: previous });
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setProviderBusy(false);
    }
  }

  async function handleLoadProviderModels() {
    if (!providerDraft || providerModelsBusy) return;
    setProviderModelsRefreshTurn((current) => current + 1);
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

  async function handlePickWorkspace() {
    if (workspacePickerBusy) return;
    setWorkspacePickerBusy(true);
    setComposerError(null);
    try {
      const nextPath = await pickWorkspaceFolder(workspaceDraft.trim() || undefined);
      if (nextPath) setWorkspaceDraft(nextPath);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setWorkspacePickerBusy(false);
    }
  }

  async function refreshWorkspaceAfterProjectSession(
    nextState: ProjectSessionState,
    prefetchedAgentState?: { sessionId: string; request: Promise<AgentState> }
  ) {
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
      setSessionLoadingId(null);
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
    setStreamResetVersion((version) => version + 1);
    if (nextWorkspaceRoot) {
      setWorkspaceDraft(nextWorkspaceRoot);
      setRuntime((current) =>
        current ? { ...current, workspaceRoot: nextWorkspaceRoot } : current
      );
    }
    if (previousSessionId !== sessionId) {
      if (sessionId) restoreCachedSessionState(sessionId);
      else {
        setAgentState(null);
        setAgentTraceState(null);
        setContextState(null);
      }
      setSelectedTraceStepId(null);
    }
    if (!sessionId) {
      setSessionLoadingId(null);
      if (workspaceChanged) refreshWorkspaceScopedState();
      return;
    }

    let nextAgentState: AgentState;
    try {
      nextAgentState = await (
        prefetchedAgentState?.sessionId === sessionId
          ? prefetchedAgentState.request
          : requestSessionAgentState(sessionId)
      );
    } catch (error) {
      reportBackgroundError(error);
      return;
    }
    if (!isCurrentRequest()) return;
    setSessionLoadingId(null);
    acknowledgeOptimisticUserMessage(sessionId, nextAgentState.messages);
    startTransition(() => {
      setAgentState((current) => {
        const merged = preserveOptimisticQueuedMessages(
          sessionId,
          mergeAgentStateSnapshot(current, nextAgentState)
        );
        return agentStateUnchanged(current, merged) ? current : merged;
      });
    });
    updateSessionStatus(sessionId, nextAgentState.status, nextAgentState.canContinue);

    if (workspaceChanged) refreshWorkspaceScopedState();
  }

  async function drainProjectSessionSelections() {
    if (sessionSelectionRunningRef.current) return;
    sessionSelectionRunningRef.current = true;
    try {
      while (sessionSelectionPendingRef.current) {
        const pending = sessionSelectionPendingRef.current;
        sessionSelectionPendingRef.current = null;
        try {
          const state = await pending.operation();
          pending.waiters.forEach(({ resolve }) => resolve(state));
        } catch (error) {
          pending.waiters.forEach(({ reject }) => reject(error));
        }
      }
    } finally {
      sessionSelectionRunningRef.current = false;
    }
  }

  function enqueueProjectSessionSelection(
    operation: () => Promise<ProjectSessionState>
  ) {
    const request = new Promise<ProjectSessionState>((resolve, reject) => {
      const pending = sessionSelectionPendingRef.current;
      if (pending) {
        pending.operation = operation;
        pending.waiters.push({ resolve, reject });
      } else {
        sessionSelectionPendingRef.current = {
          operation,
          waiters: [{ resolve, reject }]
        };
      }
    });
    void drainProjectSessionSelections();
    return request;
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
    deleted.forEach((sessionId) => {
      trackedSessionTaskIdsRef.current.delete(sessionId);
      agentStateRevisionsRef.current.delete(sessionId);
      agentStateCacheRef.current.delete(sessionId);
      agentTraceCacheRef.current.delete(sessionId);
      contextStateCacheRef.current.delete(sessionId);
      agentStateRequestsRef.current.delete(sessionId);
    });
  }

  function showWorkspaceView(view: Exclude<WorkspaceView, "settings">) {
    const leavingSettings = activeView === "settings";
    const leavingSchedule = activeView === "schedule" && view === "timeline";
    const enteringSchedule = activeView !== "schedule" && view === "schedule";
    if (enteringSchedule) {
      if (activeView === "timeline") setInspectorOpenBeforeSchedule(inspectorOpen);
      setInspectorOpen(false);
    }
    setWorkspaceViewBeforeSettings(view);
    setActiveView(view);
    if (leavingSettings && view === "timeline") {
      setInspectorOpen(inspectorOpenBeforeSettings);
    } else if (leavingSchedule) {
      setInspectorOpen(inspectorOpenBeforeSchedule);
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
      setInspectorOpenBeforeSettings(
        activeView === "schedule" ? inspectorOpenBeforeSchedule : inspectorOpen
      );
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
    showTimelineView();
    if (projectId === projectSessionState?.activeProjectId) return;
    const selectionRequest = ++sessionSelectionRequestRef.current;
    const targetSession = projectSessionState?.sessions.find(
      (session) => session.projectId === projectId && !session.archived
    );
    activeSessionIdRef.current = targetSession?.id ?? null;
    if (targetSession) acknowledgeSessionResult(targetSession.id);
    setComposerError(null);
    setProjectSessionState((current) => {
      if (!current || !current.projects.some((project) => project.id === projectId)) {
        return current;
      }
      const nextSession =
        current.sessions.find(
          (session) =>
            session.id === current.activeSessionId &&
            session.projectId === projectId &&
            !session.archived
        ) ??
        current.sessions.find(
          (session) => session.projectId === projectId && !session.archived
        );
      return {
        ...current,
        activeProjectId: projectId,
        activeSessionId: nextSession?.id ?? "",
        projects: current.projects.map((project) => ({
          ...project,
          active: project.id === projectId
        })),
        sessions: current.sessions.map((session) => ({
          ...session,
          active: session.id === nextSession?.id
        }))
      };
    });
    if (targetSession) restoreCachedSessionState(targetSession.id);
    else {
      setSessionLoadingId(null);
      setAgentState(null);
      setAgentTraceState(null);
      setContextState(null);
    }
    setSelectedTraceStepId(null);
    setSelectedThreadItem(null);
    setStreamResetVersion((version) => version + 1);
    const agentStateRequest = targetSession
      ? requestSessionAgentState(targetSession.id)
      : null;
    if (targetSession && agentStateRequest) {
      applySelectedSessionAgentState(targetSession.id, selectionRequest, agentStateRequest);
    }
    try {
      const next = await enqueueProjectSessionSelection(() => selectProject(projectId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(
        next,
        targetSession && agentStateRequest
          ? { sessionId: targetSession.id, request: agentStateRequest }
          : undefined
      );
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
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
    restoreCachedSessionState(sessionId);
    setSelectedTraceStepId(null);
    setSelectedThreadItem(null);
    setStreamResetVersion((version) => version + 1);
    const agentStateRequest = requestSessionAgentState(sessionId);
    applySelectedSessionAgentState(sessionId, selectionRequest, agentStateRequest);
    try {
      const next = await enqueueProjectSessionSelection(() => selectSession(sessionId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(next, {
        sessionId,
        request: agentStateRequest
      });
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    }
  }

  async function handleOpenScheduledSession(sessionId: string) {
    if (sessionId !== activeSessionIdRef.current) {
      await handleSelectSession(sessionId);
      return;
    }
    const selectionRequest = ++sessionSelectionRequestRef.current;
    showTimelineView();
    setSessionLoadingId(sessionId);
    setComposerError(null);
    try {
      const next = await enqueueProjectSessionSelection(() => selectSession(sessionId));
      if (selectionRequest !== sessionSelectionRequestRef.current) return;
      await refreshWorkspaceAfterProjectSession(next);
    } catch (error) {
      if (selectionRequest === sessionSelectionRequestRef.current) {
        setSessionLoadingId(null);
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

  async function handleSessionEffortChange(effort: AgentEffort) {
    const sessionId = activeSession?.id;
    if (!sessionId || effort === agentEffort) return;
    const previousEffort = agentEffort;
    setProjectSessionState((current) =>
      current
        ? {
            ...current,
            sessions: current.sessions.map((session) =>
              session.id === sessionId ? { ...session, effort } : session
            )
          }
        : current
    );
    try {
      const next = await setSessionEffort(sessionId, effort);
      const persisted = next.sessions.find((session) => session.id === sessionId)?.effort ?? effort;
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === sessionId ? { ...session, effort: persisted } : session
              )
            }
          : next
      );
    } catch (error) {
      setProjectSessionState((current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((session) =>
                session.id === sessionId ? { ...session, effort: previousEffort } : session
              )
            }
          : current
      );
      setComposerError(error instanceof Error ? error.message : String(error));
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
      const next = await archiveSession(sessionId);
      clearSessionTransientStatus(sessionId);
      await refreshWorkspaceAfterProjectSession(next);
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
      clearSessionTransientStatus(sessionId);
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

  async function refineAutomaticSessionTitle(
    sessionId: string,
    prompt: string,
    answer: string
  ) {
    const firstRoundTitle = sessionTitleFromFirstRound(prompt, answer);
    setProjectSessionState((current) =>
      current
        ? {
            ...current,
            sessions: current.sessions.map((session) =>
              session.id === sessionId ? { ...session, name: firstRoundTitle } : session
            )
          }
        : current
    );
    setAgentState((current) =>
      current?.sessionId === sessionId
        ? { ...current, sessionName: firstRoundTitle }
        : current
    );
    const nextState = await generateSessionTitle(sessionId, prompt, answer);
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

  function applyAgentStateForSession(sessionId: string, next: AgentState) {
    const effectiveNext = preserveOptimisticQueuedMessages(sessionId, next);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: effectiveNext.eventCount,
      latestSequence: effectiveNext.latestSequence,
      latestTimestampMs: 0
    });
    rememberSessionState(agentStateCacheRef.current, sessionId, effectiveNext);
    acknowledgeOptimisticUserMessage(sessionId, effectiveNext.messages);
    updateSessionStatus(sessionId, effectiveNext.status, effectiveNext.canContinue);
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => mergeAgentStateSnapshot(current, effectiveNext));
    }
  }

  function updateQueuedMessagesForSession(
    sessionId: string,
    update: (messages: QueuedAgentMessage[]) => QueuedAgentMessage[]
  ) {
    const updateState = (current: AgentState) => {
      const queuedMessages = update(current.queuedMessages);
      return queuedMessages === current.queuedMessages
        ? current
        : { ...current, queuedMessages };
    };
    const cached = agentStateCacheRef.current.get(sessionId);
    if (cached) {
      rememberSessionState(agentStateCacheRef.current, sessionId, updateState(cached));
    }
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = updateState(current);
        rememberSessionState(agentStateCacheRef.current, sessionId, next);
        return next;
      });
    }
  }

  function queuedMessageForSession(sessionId: string, queueId: string) {
    const cached = agentStateCacheRef.current.get(sessionId);
    const state = agentState?.sessionId === sessionId ? agentState : cached;
    return state?.queuedMessages.find((message) => message.id === queueId) ?? null;
  }

  function releaseSteeredQueuedMessagesForSession(sessionId: string) {
    steeredQueuedMessageIdsRef.current.forEach((queueId) => {
      if (optimisticallyDeletedQueuedMessagesRef.current.get(queueId) !== sessionId) return;
      steeredQueuedMessageIdsRef.current.delete(queueId);
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
    });
  }

  function applyQueuedMessageReceiptForSession(
    sessionId: string,
    receipt: QueuedAgentMessageReceipt
  ) {
    const revision = agentStateRevisionsRef.current.get(sessionId);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: Math.max(revision?.eventCount ?? 0, receipt.eventCount),
      latestSequence: Math.max(revision?.latestSequence ?? 0, receipt.latestSequence),
      latestTimestampMs: Math.max(
        revision?.latestTimestampMs ?? 0,
        receipt.latestTimestampMs
      )
    });
    const mergeReceipt = (current: AgentState) => ({
      ...current,
      eventCount: Math.max(current.eventCount, receipt.eventCount),
      latestSequence: Math.max(current.latestSequence, receipt.latestSequence),
      queuedMessages: mergeQueuedAgentMessage(current.queuedMessages, receipt.message)
    });
    const cached = agentStateCacheRef.current.get(sessionId);
    if (cached) rememberSessionState(agentStateCacheRef.current, sessionId, mergeReceipt(cached));
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = mergeReceipt(current);
        rememberSessionState(agentStateCacheRef.current, sessionId, next);
        return next;
      });
    }
  }

  function applyQueuedMessageActionReceiptForSession(
    sessionId: string,
    receipt: QueuedAgentMessageActionReceipt
  ) {
    const revision = agentStateRevisionsRef.current.get(sessionId);
    agentStateRevisionsRef.current.set(sessionId, {
      eventCount: Math.max(revision?.eventCount ?? 0, receipt.eventCount),
      latestSequence: Math.max(revision?.latestSequence ?? 0, receipt.latestSequence),
      latestTimestampMs: Math.max(
        revision?.latestTimestampMs ?? 0,
        receipt.latestTimestampMs
      )
    });
    const applyReceipt = (current: AgentState) => {
      const queuedMessages = receipt.message
        ? mergeQueuedAgentMessage(current.queuedMessages, receipt.message)
        : current.queuedMessages.filter((message) => message.id !== receipt.queueId);
      return {
        ...current,
        status: receipt.cancelledActiveRun ? "cancelled" : current.status,
        canCancel: receipt.cancelledActiveRun ? false : current.canCancel,
        canRetry: receipt.cancelledActiveRun ? true : current.canRetry,
        eventCount: Math.max(current.eventCount, receipt.eventCount),
        latestSequence: Math.max(current.latestSequence, receipt.latestSequence),
        queuedMessages
      };
    };
    const cached = agentStateCacheRef.current.get(sessionId);
    if (cached) rememberSessionState(agentStateCacheRef.current, sessionId, applyReceipt(cached));
    if (activeSessionIdRef.current === sessionId) {
      setAgentState((current) => {
        if (!current || current.sessionId !== sessionId) return current;
        const next = applyReceipt(current);
        rememberSessionState(agentStateCacheRef.current, sessionId, next);
        return next;
      });
    }
  }

  async function drainQueuedMessages(sessionId: string) {
    if (
      queueDrainingSessionIdsRef.current.has(sessionId) ||
      suppressQueueDrainSessionIdsRef.current.has(sessionId)
    ) {
      return;
    }
    queueDrainingSessionIdsRef.current.add(sessionId);
    markSessionBusy(sessionId, true);
    try {
      while (!suppressQueueDrainSessionIdsRef.current.has(sessionId)) {
        markSessionTaskStarted(sessionId);
        const next = await runNextQueuedAgentMessage(sessionId);
        releaseSteeredQueuedMessagesForSession(sessionId);
        if (!next) {
          trackedSessionTaskIdsRef.current.delete(sessionId);
          break;
        }
        applyAgentStateForSession(sessionId, next);
        if (activeSessionIdRef.current === sessionId) {
          setComposerError(next.lastError);
          setStreamResetVersion((version) => version + 1);
        }
        if (
          next.queuedMessages.length === 0 ||
          next.status !== "completed" ||
          next.canContinue ||
          next.pendingApprovals.length > 0 ||
          Boolean(next.lastError)
        ) {
          break;
        }
      }
      setProjectSessionState(await getProjectSessionState());
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      releaseSteeredQueuedMessagesForSession(sessionId);
      updateSessionStatus(sessionId, "failed");
      const message = error instanceof Error ? error.message : String(error);
      try {
        const failedState = await getAgentState(sessionId);
        applyAgentStateForSession(sessionId, failedState);
      } catch {
        // Preserve the original queue error when a follow-up state read also fails.
      }
      if (activeSessionIdRef.current === sessionId) setComposerError(message);
    } finally {
      queueDrainingSessionIdsRef.current.delete(sessionId);
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
    }
  }

  async function handleEditQueuedMessage(queueId: string, prompt: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) {
      const error = new Error("queued message not found");
      setComposerError(error.message);
      throw error;
    }
    const optimistic = { ...previous, prompt: prompt.trim(), updatedAtMs: Date.now() };
    optimisticQueuedMessagesRef.current.set(queueId, optimistic);
    updateQueuedMessagesForSession(sessionId, (messages) =>
      mergeQueuedAgentMessage(messages, optimistic)
    );
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await editQueuedAgentMessage(sessionId, queueId, prompt);
      optimisticQueuedMessagesRef.current.delete(queueId);
      applyQueuedMessageActionReceiptForSession(sessionId, receipt);
    } catch (error) {
      optimisticQueuedMessagesRef.current.delete(queueId);
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, previous)
      );
      setComposerError(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleDeleteQueuedMessage(queueId: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) return;
    optimisticallyDeletedQueuedMessagesRef.current.set(queueId, sessionId);
    updateQueuedMessagesForSession(sessionId, (messages) =>
      messages.filter((message) => message.id !== queueId)
    );
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await deleteQueuedAgentMessage(sessionId, queueId);
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      applyQueuedMessageActionReceiptForSession(sessionId, receipt);
    } catch (error) {
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, previous)
      );
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleSteerQueuedMessage(queueId: string) {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    const previous = queuedMessageForSession(sessionId, queueId);
    if (!previous) return;
    optimisticallyDeletedQueuedMessagesRef.current.set(queueId, sessionId);
    steeredQueuedMessageIdsRef.current.add(queueId);
    updateQueuedMessagesForSession(sessionId, (messages) =>
      messages.filter((message) => message.id !== queueId)
    );
    const runCommandActive = busySessionIds.has(sessionId);
    suppressQueueDrainSessionIdsRef.current.delete(sessionId);
    setQueuedMessageBusyId(queueId);
    setComposerError(null);
    try {
      const receipt = await steerQueuedAgentMessage(sessionId, queueId);
      applyQueuedMessageActionReceiptForSession(sessionId, { ...receipt, message: null });
      if (!runCommandActive) void drainQueuedMessages(sessionId);
    } catch (error) {
      steeredQueuedMessageIdsRef.current.delete(queueId);
      optimisticallyDeletedQueuedMessagesRef.current.delete(queueId);
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, previous)
      );
      setComposerError(error instanceof Error ? error.message : String(error));
    } finally {
      setQueuedMessageBusyId(null);
    }
  }

  async function handleSendPrompt(value: string) {
    const nextPrompt = value.trim();
    const sessionId = activeSession?.id;
    const attachments = sessionId ? attachmentDrafts[sessionId] ?? [] : [];
    if (
      (!nextPrompt && attachments.length === 0) ||
      !sessionId ||
      attachmentBusySessionIds.has(sessionId)
    ) {
      return;
    }
    const visiblePrompt =
      nextPrompt || `Review attached ${attachments.map((attachment) => attachment.name).join(", ")}`;
    const sessionAgentState =
      activeAgentState?.sessionId === sessionId
        ? activeAgentState
        : agentStateCacheRef.current.get(sessionId);
    if (
      busySessionIds.has(sessionId) ||
      sessionAgentState?.status === "running" ||
      sessionAgentState?.status === "waiting_for_permission" ||
      sessionAgentState?.canCancel
    ) {
      setComposerError(null);
      const queueId = queuedMessageClientId();
      const queuedAt = Date.now();
      const optimisticMessage: QueuedAgentMessage = {
        id: queueId,
        sessionId,
        prompt: visiblePrompt,
        attachments,
        effort: agentEffort,
        mode: "queue",
        createdAtMs: queuedAt,
        updatedAtMs: queuedAt
      };
      optimisticQueuedMessagesRef.current.set(queueId, optimisticMessage);
      setPersistingQueuedMessageIds((current) => {
        const next = new Set(current);
        next.add(queueId);
        return next;
      });
      updateQueuedMessagesForSession(sessionId, (messages) =>
        mergeQueuedAgentMessage(messages, optimisticMessage)
      );
      setAttachmentDrafts((current) => ({ ...current, [sessionId]: [] }));
      try {
        const receipt = await queueAgentMessage(
          nextPrompt,
          sessionId,
          attachments,
          agentEffort,
          queueId
        );
        optimisticQueuedMessagesRef.current.delete(queueId);
        applyQueuedMessageReceiptForSession(sessionId, receipt);
      } catch (error) {
        optimisticQueuedMessagesRef.current.delete(queueId);
        updateQueuedMessagesForSession(sessionId, (messages) =>
          messages.filter((message) => message.id !== queueId)
        );
        if (attachments.length > 0) {
          setAttachmentDrafts((current) =>
            (current[sessionId] ?? []).length > 0
              ? current
              : { ...current, [sessionId]: attachments }
          );
        }
        setComposerDrafts((current) => ({
          ...current,
          [sessionId]: current[sessionId]?.trim() ? current[sessionId] : value
        }));
        setComposerError(error instanceof Error ? error.message : String(error));
      } finally {
        setPersistingQueuedMessageIds((current) => {
          if (!current.has(queueId)) return current;
          const next = new Set(current);
          next.delete(queueId);
          return next;
        });
      }
      return;
    }
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
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    setAttachmentDrafts((current) => ({ ...current, [sessionId]: [] }));
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    const submittedAt = Date.now();
    const optimisticUserMessage: ChatMessageView = {
      role: "user",
      content: visiblePrompt,
      timestampMs: submittedAt,
      attachments
    };
    optimisticUserMessagesRef.current.set(sessionId, optimisticUserMessage);
    setOptimisticUserMessageRevision((revision) => revision + 1);
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
    let completedState: AgentState | null = null;
    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const next = await runAgentTask(nextPrompt, sessionId, attachments, agentEffort);
      completedState = next;
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status, next.canContinue);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, next)
          )
        );
        setComposerError(next.lastError);
        setStreamResetVersion((version) => version + 1);
      }
      setProjectSessionState(await getProjectSessionState());
      const firstRoundAnswer = [...next.messages]
        .reverse()
        .find((message) => message.role === "assistant" && message.content.trim())
        ?.content.trim();
      if (automaticSessionId && firstRoundAnswer) {
        void refineAutomaticSessionTitle(
          automaticSessionId,
          visiblePrompt,
          firstRoundAnswer
        );
      }
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
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, failedState)
          )
        );
      }
    } finally {
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
      const suppressDrain = suppressQueueDrainSessionIdsRef.current.delete(sessionId);
      if (
        !suppressDrain &&
        completedState &&
        completedState.queuedMessages.length > 0 &&
        !completedState.canContinue &&
        !completedState.lastError &&
        (completedState.status === "completed" || completedState.status === "cancelled")
      ) {
        void drainQueuedMessages(sessionId);
      }
    }
  }

  async function handleCancelAgentTask() {
    const sessionId = activeSession?.id;
    if (!sessionId) return;
    suppressQueueDrainSessionIdsRef.current.add(sessionId);
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    try {
      const next = await cancelAgentTask(sessionId);
      acknowledgeOptimisticUserMessage(sessionId, next.messages);
      updateSessionStatus(sessionId, next.status, next.canContinue);
      markSessionBusy(sessionId, false);
      if (activeSessionIdRef.current === sessionId) {
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, next)
          )
        );
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
    setStreamResetVersion((version) => version + 1);
    setComposerError(null);
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    let completedState: AgentState | null = null;
    try {
      const next = await retryAgentTask(sessionId);
      completedState = next;
      applyAgentStateForSession(sessionId, next);
      if (activeSessionIdRef.current === sessionId) setComposerError(next.lastError);
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      markSessionBusy(sessionId, false);
      if (
        completedState?.status === "completed" &&
        !completedState.canContinue &&
        !completedState.lastError &&
        completedState.queuedMessages.length > 0
      ) {
        void drainQueuedMessages(sessionId);
      }
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
    decision: "allow_once" | "allow_for_session" | "deny",
    targetSessionId = activeSession?.id
  ) {
    const sessionId = targetSessionId;
    if (!sessionId || busySessionIds.has(sessionId)) return;
    markSessionTaskStarted(sessionId);
    markSessionBusy(sessionId, true);
    setComposerError(null);
    if (activeSessionIdRef.current === sessionId) {
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
    }
    let completedState: AgentState | null = null;
    try {
      const next = await resolveAgentPermission(requestId, decision, sessionId);
      completedState = next;
      applyAgentStateForSession(sessionId, next);
      if (activeSessionIdRef.current === sessionId) setComposerError(next.lastError);
      await refreshAgentTrace(true, sessionId);
    } catch (error) {
      updateSessionStatus(sessionId, "failed");
      if (activeSessionIdRef.current === sessionId) {
        setComposerError(error instanceof Error ? error.message : String(error));
        const failedState = await getAgentState(sessionId);
        setAgentState((current) =>
          preserveOptimisticQueuedMessages(
            sessionId,
            mergeAgentStateSnapshot(current, failedState)
          )
        );
      }
    } finally {
      markSessionBusy(sessionId, false);
      void refreshPermissionReviews().catch(() => {});
      if (
        completedState?.status === "completed" &&
        !completedState.canContinue &&
        !completedState.lastError &&
        completedState.queuedMessages.length > 0
      ) {
        void drainQueuedMessages(sessionId);
      }
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
      if (next.exportPath) {
        await revealArtifact(next.exportPath);
      }
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

  useEffect(() => {
    const sessionId = activeAgentState?.sessionId;
    if (
      !sessionId ||
      activeSessionBusy ||
      activeAgentState.queuedMessages.length === 0 ||
      activeAgentState.canContinue ||
      activeAgentState.pendingApprovals.length > 0 ||
      !["idle", "completed"].includes(activeAgentState.status)
    ) {
      return;
    }
    void drainQueuedMessages(sessionId);
  }, [
    activeAgentState?.canContinue,
    activeAgentState?.pendingApprovals.length,
    activeAgentState?.queuedMessages.length,
    activeAgentState?.sessionId,
    activeAgentState?.status,
    activeSessionBusy
  ]);

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
        )}
        {activeView === "timeline" && (
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

      <div className="window-workspace-header">
        <div className="topbar-title">
          <div>
            <h1>
              {activeView === "settings"
                ? "Settings"
                : activeView === "schedule"
                  ? "Schedule"
                  : activeSession?.name ?? "Session"}
            </h1>
          </div>
        </div>
        {activeView === "timeline" && <div className="topbar-actions">
            <div
              className="context-usage"
              title={`${activeAgentState?.contextTokensUsed ?? 0} of ${
                activeAgentState?.contextWindowTokens ??
                phase4?.provider.contextWindowTokens ??
                128000
              } context tokens${activeAgentState?.contextUsageEstimated ? " (estimated)" : ""}`}
            >
              <span>
                {activeAgentState?.contextUsageEstimated ? "~" : ""}
                {formatTokenCount(activeAgentState?.contextTokensUsed ?? 0)} tokens
              </span>
              <strong>
                {Math.round(activeAgentState?.contextRemainingPercent ?? 100)}% left
              </strong>
              <progress
                max={100}
                value={activeAgentState?.contextRemainingPercent ?? 100}
                aria-label="Context window remaining"
              />
            </div>
            <div className={`runtime-pill ${statusText === "Ready" ? "ready" : ""}`}>
              <CheckCircle2 size={16} aria-hidden="true" />
              <span>{statusText}</span>
            </div>
        </div>}
      </div>

      {activeView !== "settings" && sidebarOpen && (
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

      {activeView !== "settings" && (
        <Sidebar
          activeView={activeView}
          projects={projects}
          sessions={sessions}
          busy={projectSessionBusy}
          searchOpen={sidebarSearchOpen}
          searchQuery={sidebarQuery}
          selectedScheduleId={selectedScheduleId}
          projectCreateOpen={projectCreateOpen}
          projectName={newProjectName}
          onViewChange={handleWorkspaceViewChange}
          onScheduleSelect={setSelectedScheduleId}
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
      )}

      <section className="workspace" data-view={activeView} aria-label="Agent workspace">
        {activeView === "timeline" ? (
          <>
            <LiveSessionThread
              sessionId={activeSession?.id ?? null}
              loading={sessionLoadingId === activeSession?.id && !activeAgentState}
              messages={visibleAgentMessages}
              timeline={activeAgentState?.timeline ?? []}
              streamResetVersion={streamResetVersion}
              status={activeAgentState?.status ?? "idle"}
              runStartedAtMs={activeAgentState?.runStartedAtMs ?? 0}
              hasOlderHistory={activeAgentState?.hasOlderHistory ?? false}
              loadingOlderHistory={loadingOlderSessionId === activeSession?.id}
              selectedId={selectedThreadItem?.id ?? null}
              onLoadOlderHistory={loadOlderAgentHistory}
              onSelect={handleThreadSelection}
              onEditMessage={handleThreadMessageEdit}
              onLinkOpenError={setComposerError}
              onStreamDone={handleAgentStreamDone}
            />

            <div
              className="composer-stack"
              data-has-queued={Boolean(activeAgentState?.queuedMessages.length)}
            >
              <QueuedMessages
                messages={activeAgentState?.queuedMessages ?? []}
                busyId={queuedMessageBusyId}
                persistingIds={persistingQueuedMessageIds}
                onSteer={handleSteerQueuedMessage}
                onEdit={handleEditQueuedMessage}
                onDelete={handleDeleteQueuedMessage}
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
                onEffortChange={(effort) => void handleSessionEffortChange(effort)}
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
            </div>
          </>
        ) : activeView === "schedule" ? (
          <Suspense fallback={<section className="schedule-view" aria-busy="true" />}>
            <ScheduleView
              projects={projectSessionState?.projects ?? []}
              sessions={projectSessionState?.sessions ?? []}
              onOpenSession={(sessionId) => void handleOpenScheduledSession(sessionId)}
              onBack={showTimelineView}
              requestedScheduleId={selectedScheduleId}
              onScheduleSelect={setSelectedScheduleId}
            />
          </Suspense>
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
                    {category.id === "permissions" && (
                      <small className="settings-tab-count">
                        {activePermissionReviews.length}
                      </small>
                    )}
                  </button>
                ))}
              </nav>
            </aside>

            <div className="settings-detail">
            {settingsCategory === "runtime" && (
              <>
            <section className="settings-section" data-settings-group="runtime">
              <div className="section-title">
                <LayoutDashboard size={17} aria-hidden="true" />
                <h2>Workspace</h2>
              </div>
              <div className="provider-form">
                <div className="workspace-folder-field">
                  <span>Folder</span>
                  <button
                    className="workspace-folder-selector"
                    type="button"
                    disabled={workspaceBusy || workspacePickerBusy}
                    aria-label="Choose workspace folder"
                    title={workspaceDraft || "Choose workspace folder"}
                    onClick={handlePickWorkspace}
                  >
                    <span>{workspaceDraft || "Choose workspace folder"}</span>
                    <FolderOpen size={16} aria-hidden="true" />
                  </button>
                </div>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={workspaceBusy || workspacePickerBusy || !workspaceDraft.trim()}
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
              </>
            )}

            {settingsCategory === "knowledge" && (
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
            )}

            {settingsCategory === "sessions" && (
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
            )}

            {settingsCategory === "models" && (
              <>
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
                      <RefreshCw
                        aria-hidden="true"
                        className={
                          providerModelsRefreshTurn > 0 ? "settings-refresh-turn" : undefined
                        }
                        key={providerModelsRefreshTurn}
                      />
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
                      <div
                        className="provider-endpoint-input"
                        data-validation={imageEndpointValidation}
                      >
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
                        {imageEndpointValidation === "valid" && (
                          <CheckCircle2
                            className="provider-endpoint-check"
                            aria-label="Image endpoint verified"
                          />
                        )}
                      </div>
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

            <section className="settings-section" data-settings-group="models">
              <div className="prompt-evolution-title">
                <div className="section-title">
                  <Dna size={17} aria-hidden="true" />
                  <h2>Genetic Pareto</h2>
                  {phase4?.promptEvolution?.evaluationInflight && (
                    <span className="prompt-evolution-running">Evaluating in background</span>
                  )}
                </div>
                {providerDraft && (
                  <label className="settings-switch">
                    <input
                      type="checkbox"
                      checked={providerDraft.promptEvolutionEnabled}
                      disabled={providerBusy}
                      onChange={(event) =>
                        void handlePromptEvolutionToggle(event.target.checked)
                      }
                    />
                    <span className="settings-switch-track" aria-hidden="true">
                      <span />
                    </span>
                    <span>{providerDraft.promptEvolutionEnabled ? "On" : "Off"}</span>
                  </label>
                )}
              </div>
              <p className="settings-section-copy">
                Candidate harnesses execute in an isolated arena before promotion. Same-task paired
                runs train the population, historical replay runs provide holdout evidence, and a
                Wilson confidence gate controls staged canary rollout with automatic rollback.
              </p>
              <div className="prompt-evolution-summary" aria-label="Evolution overview">
                <span><strong>{phase4?.promptEvolution?.observedRuns ?? 0}</strong> observed</span>
                <span><strong>{phase4?.promptEvolution?.pairedRuns ?? 0}</strong> paired</span>
                <span><strong>{phase4?.promptEvolution?.replayRuns ?? 0}</strong> replay</span>
                <span><strong>{phase4?.promptEvolution?.reflectionPackets ?? 0}</strong> reflections</span>
                <span><strong>{phase4?.promptEvolution?.learnedProfiles ?? 0}</strong> learned</span>
                <span><strong>{phase4?.promptEvolution?.populationSize ?? 0}</strong> profiles</span>
                <span><strong>{phase4?.promptEvolution?.generation ?? 0}</strong> generation</span>
                <span><strong>{phase4?.promptEvolution?.frontierProfiles ?? 0}</strong> frontier</span>
              </div>
              <div className="prompt-evolution-table-wrap">
                <table className="prompt-evolution-table">
                  <caption>Rollout by effort</caption>
                  <thead>
                    <tr>
                      <th scope="col">Effort</th>
                      <th scope="col">State</th>
                      <th scope="col">Evidence</th>
                      <th scope="col">Score</th>
                      <th scope="col">Confidence</th>
                      <th scope="col">Progress</th>
                      <th scope="col">Rollbacks</th>
                      <th scope="col">Next</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(phase4?.promptEvolution?.efforts ?? []).map((effort) => (
                      <tr key={effort.effort} title={effort.championId ?? undefined}>
                        <th scope="row">{promptEvolutionProfileLabel(effort.effort)}</th>
                        <td>
                          <span
                            className={`prompt-evolution-status ${effort.evaluationInflight ? "evaluating" : effort.rolloutStatus}`}
                          >
                            {promptEvolutionEffortStatus(effort)}
                          </span>
                        </td>
                        <td title={`${effort.reflectionPackets} feedback reflections · ${effort.learnedProfiles} learned profiles`}>
                          {effort.pairedRuns}/3 · {effort.replayRuns}/3 · R{effort.reflectionPackets}
                        </td>
                        <td>{effort.championScore === null ? "-" : `${Math.round(effort.championScore * 100)}%`}</td>
                        <td>{effort.promotionConfidence === null ? "-" : `${Math.round(effort.promotionConfidence * 100)}%`}</td>
                        <td title={`Ready ${effort.readyProfiles} · Stagnant ${effort.stagnantGenerations}/3`}>
                          Gen {effort.evaluatedGenerations} · Ready {effort.readyProfiles}
                        </td>
                        <td>{effort.rollbackCount}</td>
                        <td title={effort.freezeReason ?? undefined}>{effort.nextMode.replace(/_/g, " ")}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="prompt-evolution-table-wrap">
                <table className="prompt-evolution-table prompt-evolution-profile-table">
                  <caption>Candidate profiles</caption>
                  <thead>
                    <tr>
                      <th scope="col">Profile</th>
                      <th scope="col">Evidence</th>
                      <th scope="col">Success</th>
                      <th scope="col">Quality</th>
                      <th scope="col">Reward</th>
                      <th scope="col">Signal</th>
                      <th scope="col">Efficiency</th>
                      <th scope="col">State</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(phase4?.promptEvolution?.profiles ?? [])
                      .filter((profile) => profile.next || profile.frontier || profile.runs > 0)
                      .map((profile) => (
                        <tr key={profile.id} title={profile.id}>
                          <th className="prompt-evolution-profile-cell" scope="row">
                            <strong>{promptEvolutionProfileLabel(profile.effort)}</strong>
                            <small>{profile.learned ? "Learned" : "Genetic"} · Gen {profile.generation}</small>
                          </th>
                          <td title={`${profile.reflectionRuns} feedback reflections`}>
                            {profile.trainRuns} / {profile.holdoutRuns} · R{profile.reflectionRuns}
                          </td>
                          <td>{profile.runs ? `${Math.round(profile.successRate * 100)}%` : "-"}</td>
                          <td>{profile.averageQuality === null ? "-" : `${Math.round(profile.averageQuality * 100)}%`}</td>
                          <td>{profile.averageReward === null ? "-" : `${Math.round(profile.averageReward * 100)}%`}</td>
                          <td className="prompt-evolution-signal-cell">
                            <span>
                              {profile.averageRelativeReward === null
                                ? "-"
                                : `${profile.averageRelativeReward >= 0 ? "+" : ""}${Math.round(profile.averageRelativeReward * 100)}%`}
                            </span>
                            <small>
                              Credit {profile.averageStepCredit === null ? "-" : `${Math.round(profile.averageStepCredit * 100)}%`}
                            </small>
                          </td>
                          <td className="prompt-evolution-efficiency-cell">
                            <span>{formatObservedDuration(profile.averageLatencyMs)}</span>
                            <small>{profile.averageTokens ? formatTokenCount(profile.averageTokens) : "-"}</small>
                          </td>
                          <td>
                            <span className={profile.frontier || profile.next ? "pareto-frontier active" : "pareto-frontier"}>
                              {promptEvolutionProfileStatus(profile)}
                            </span>
                          </td>
                        </tr>
                      ))}
                  </tbody>
                </table>
              </div>
            </section>
              </>
            )}

            {settingsCategory === "agent" && (
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
            )}

            {settingsCategory === "permissions" && (
            <section className="settings-section" data-settings-group="permissions">
              <div className="permission-review-heading">
                <div className="section-title">
                  <ShieldCheck size={17} aria-hidden="true" />
                  <h2>Pending Reviews</h2>
                </div>
                <span>{activePermissionReviews.length}</span>
              </div>
              <p className="settings-section-copy">
                Review actions that can modify files, run processes, use the network, or access
                sensitive context. Ignored requests remain paused until restored.
              </p>
              {activePermissionReviews.length === 0 ? (
                <div className="permission-review-empty">
                  <CheckCircle2 aria-hidden="true" />
                  <span>No actions are waiting for review.</span>
                </div>
              ) : (
                <div className="permission-review-list" aria-label="Pending permission reviews">
                  {activePermissionReviews.map((review) => {
                    const sessionBusy = Boolean(
                      review.sessionId && busySessionIds.has(review.sessionId)
                    );
                    return (
                      <article className="permission-review-row" key={review.requestId}>
                        <header>
                          <div>
                            <strong>{review.action}</strong>
                            <span>{permissionReviewSourceLabel(review.source)}</span>
                          </div>
                          <em data-risk={review.risk}>{review.risk}</em>
                        </header>
                        <div className="permission-review-context">
                          <strong title={review.sessionId ?? undefined}>
                            {review.sessionName ?? "No related session"}
                          </strong>
                          <span>
                            {review.projectName ?? "Cindx"} · {formatTime(review.requestedAtMs)}
                          </span>
                        </div>
                        <p>{review.reason}</p>
                        <dl className="permission-review-meta">
                          <div>
                            <dt>Scope</dt>
                            <dd>{review.scope || "Current workspace"}</dd>
                          </div>
                        </dl>
                        {review.input && (
                          <details className="permission-review-input">
                            <summary>
                              <span>Request details</span>
                              <SettingsChevron />
                            </summary>
                            <pre>{review.input}</pre>
                          </details>
                        )}
                        <div className="permission-review-actions">
                          <button
                            className="permission-approve"
                            type="button"
                            disabled={permissionBusy || sessionBusy}
                            onClick={() =>
                              void handleResolvePermissionReview(review, "allow_once")
                            }
                          >
                            <CheckCircle2 aria-hidden="true" />
                            <span>Approve once</span>
                          </button>
                          {review.canAllowSession && (
                            <button
                              type="button"
                              disabled={permissionBusy || sessionBusy}
                              onClick={() =>
                                void handleResolvePermissionReview(
                                  review,
                                  "allow_for_session"
                                )
                              }
                            >
                              <ShieldCheck aria-hidden="true" />
                              <span>Allow session</span>
                            </button>
                          )}
                          <button
                            type="button"
                            disabled={permissionBusy || sessionBusy}
                            onClick={() => void handleResolvePermissionReview(review, "deny")}
                          >
                            <XCircle aria-hidden="true" />
                            <span>Reject</span>
                          </button>
                          <button
                            type="button"
                            disabled={permissionBusy}
                            onClick={() => handleIgnorePermissionReview(review.requestId)}
                          >
                            <EyeOff aria-hidden="true" />
                            <span>Ignore</span>
                          </button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
              {ignoredPermissionReviews.length > 0 && (
                <details className="permission-ignored-reviews">
                  <summary>
                    <span>Ignored for now ({ignoredPermissionReviews.length})</span>
                    <SettingsChevron />
                  </summary>
                  <div>
                    {ignoredPermissionReviews.map((review) => (
                      <div className="permission-ignored-row" key={review.requestId}>
                        <span>
                          <strong>{review.action}</strong>
                          <small>{review.sessionName ?? permissionReviewSourceLabel(review.source)}</small>
                        </span>
                        <button
                          type="button"
                          onClick={() => handleRestorePermissionReview(review.requestId)}
                        >
                          <RefreshCw aria-hidden="true" />
                          <span>Restore</span>
                        </button>
                      </div>
                    ))}
                  </div>
                </details>
              )}
            </section>
            )}

            {settingsCategory === "knowledge" && (
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
              <div className="rag-stats" aria-label="Project memory stats">
                <div>
                  <strong>{phase7?.memory.records ?? 0}</strong>
                  <span>Memories</span>
                </div>
                <div>
                  <strong>{phase7?.memory.requirements ?? 0}</strong>
                  <span>Requirements</span>
                </div>
                <div>
                  <strong>{phase7?.memory.evidence ?? 0}</strong>
                  <span>Evidence</span>
                </div>
                <div>
                  <strong>{phase7?.memory.recalls ?? 0} / {phase7?.memory.observedUses ?? 0}</strong>
                  <span>Recall / use</span>
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
              <details
                className="advanced-settings knowledge-graph-details"
                onToggle={(event) => setKnowledgeGraphOpen(event.currentTarget.open)}
              >
                <summary>
                  <span className="settings-summary-label">
                    <strong>Graph Explorer</strong>
                    <SettingsChevron />
                  </span>
                  <span className="settings-summary-meta">
                    {phase7?.graph.totalNodes ?? 0} nodes · {phase7?.graph.totalEdges ?? 0} edges
                  </span>
                </summary>
                {knowledgeGraphOpen ? (
                  <Suspense
                    fallback={
                      <div className="knowledge-graph-empty">Loading graph...</div>
                    }
                  >
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
                  </Suspense>
                ) : null}
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
                    <span className="settings-summary-label">
                      <strong>Retrieval trace</strong>
                      <SettingsChevron />
                    </span>
                    <span className="settings-summary-meta">
                      {phase7.retrievalTrace.selectedCount} selected · {phase7.retrievalTrace.durationMs} ms · {phase7.retrievalTrace.indexCacheHit
                        ? "index cached"
                        : `index ${phase7.retrievalTrace.indexDurationMs} ms`}
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
            )}

            {settingsCategory === "tools" && (
              <>
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
                  <span>Registered tools</span>
                  <SettingsChevron />
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
                  <span>Manual browser controls</span>
                  <SettingsChevron />
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
                <Wrench size={17} aria-hidden="true" />
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
                  <span>Manual tool runner</span>
                  <SettingsChevron />
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
                  <span>{toolBusy ? "Running" : "Run tool"}</span>
                  <SettingsChevron action />
                </button>
                </div>
              </details>
            </section>
              </>
            )}

            {settingsCategory === "mcp" && (
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
                  <span>Add stdio server</span>
                  <SettingsChevron />
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
            )}

            {settingsCategory === "skills" && (
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
                    className={skillRefreshTurn > 0 ? "settings-refresh-turn" : undefined}
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
            )}

            {settingsCategory === "personalization" && (
              <>
            <section className="settings-section" data-settings-group="personalization">
              <div className="section-title">
                <UserRound size={17} aria-hidden="true" />
                <h2>You and Cindx</h2>
              </div>
              <div className="provider-form personalization-form">
                <label>
                  <span>What should Cindx call you?</span>
                  <input
                    value={personalizationDraft.preferredName}
                    maxLength={80}
                    autoComplete="name"
                    placeholder="Name or preferred form of address"
                    onBlur={() => void flushPersonalization(false)}
                    onChange={(event) =>
                      updatePersonalizationDraft({
                        ...personalizationDraft,
                        preferredName: event.target.value
                      })
                    }
                  />
                </label>
                <label className="personalization-select-field">
                  <span>Response tone</span>
                  <select
                    value={personalizationDraft.responseTone}
                    onChange={(event) =>
                      updatePersonalizationDraft({
                        ...personalizationDraft,
                        responseTone: event.target.value as PersonalizationConfig["responseTone"]
                      })
                    }
                  >
                    <option value="natural">Natural</option>
                    <option value="warm">Warm</option>
                    <option value="professional">Professional</option>
                    <option value="direct">Direct</option>
                  </select>
                </label>
                <fieldset className="settings-choice-field">
                  <legend>Response length</legend>
                  <div className="settings-segmented" role="group" aria-label="Response length">
                    {([
                      ["concise", "Concise"],
                      ["balanced", "Balanced"],
                      ["detailed", "Detailed"]
                    ] as const).map(([value, label]) => (
                      <button
                        className={personalizationDraft.responseLength === value ? "active" : ""}
                        type="button"
                        key={value}
                        aria-pressed={personalizationDraft.responseLength === value}
                        onClick={() =>
                          updatePersonalizationDraft({
                            ...personalizationDraft,
                            responseLength: value
                          })
                        }
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                </fieldset>
                {personalizationError && (
                  <div className="settings-inline-error">{personalizationError}</div>
                )}
                <button
                  className="secondary-button personalization-save"
                  type="button"
                  disabled={personalizationBusy}
                  onClick={() => void handleSavePersonalization()}
                >
                  <Save size={17} aria-hidden="true" />
                  <span>{personalizationBusy ? "Saving" : "Save personalization"}</span>
                </button>
              </div>
            </section>

            <section className="settings-section" data-settings-group="personalization">
              <div className="section-title">
                <Monitor size={17} aria-hidden="true" />
                <h2>Appearance</h2>
              </div>
              <div className="appearance-options" role="group" aria-label="Appearance">
                {([
                  ["light", "Light", Sun],
                  ["dark", "Dark", Moon],
                  ["system", "System", Monitor]
                ] as const).map(([value, label, Icon]) => (
                  <button
                    className={appearanceMode === value ? "active" : ""}
                    type="button"
                    key={value}
                    aria-pressed={appearanceMode === value}
                    onClick={() => handleAppearanceModeChange(value)}
                  >
                    <Icon aria-hidden="true" />
                    <span>{label}</span>
                  </button>
                ))}
              </div>
            </section>
              </>
            )}

            {settingsCategory === "about" && (
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
            )}
            </div>
          </section>
        )}
      </section>

      <Inspector
        open={activeView === "timeline" && inspectorOpen}
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
        roleSummaries={agentTraceState?.roleSummaries ?? []}
        workspaceRoot={runtime?.workspaceRoot ?? ""}
        agentStatus={activeAgentState?.status ?? "idle"}
        agentTurnCount={activeAgentState?.turnCount ?? 0}
        agentMaxTurns={activeAgentState?.maxTurns ?? 24}
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
