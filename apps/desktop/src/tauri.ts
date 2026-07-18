import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import desktopPackage from "../package.json";

export const DESKTOP_VERSION = desktopPackage.version;

export type AgentEffort = "fast" | "auto" | "pro";

export type RuntimeStatus = {
  appVersion: string;
  kernelStatus: string;
  workspaceRoot: string;
  orchestrationModes: string[];
  registeredTools: string[];
};

export type SidecarEndpointState = {
  path: string;
  exists: boolean;
  executable: boolean;
  healthy: boolean;
  healthOutput: string;
  envKey: string;
};

export type SidecarState = {
  browser: SidecarEndpointState;
  computer: SidecarEndpointState;
  autoConfigure: boolean;
  lastError: string | null;
};

export type SidecarConfigInput = {
  browserPath: string;
  computerPath: string;
  autoConfigure: boolean;
};

export type WebSearchConfigState = {
  endpoint: string;
  apiKeySet: boolean;
  configured: boolean;
};

export type WebSearchConfigInput = {
  endpoint: string;
  apiKey: string;
};

export type McpTransportConfig =
  | {
      type: "stdio";
      command: string;
      args: string[];
      env: Record<string, string>;
    }
  | {
      type: "streamable_http";
      url: string;
      headers: Record<string, string>;
    };

export type McpServerConfig = {
  id: string;
  name: string;
  enabled: boolean;
  requireApproval: boolean;
  timeoutMs: number;
  transport: McpTransportConfig;
};

export type McpServerView = {
  id: string;
  name: string;
  enabled: boolean;
  requireApproval: boolean;
  timeoutMs: number;
  transportType: "stdio" | "streamable_http";
  command: string | null;
  args: string[];
  url: string | null;
  secretKeys: string[];
  toolCount: number;
  refreshedAtMs: number | null;
  lastError: string | null;
};

export type McpState = {
  servers: McpServerView[];
  lastError: string | null;
};

export type SkillRecord = {
  id: string;
  name: string;
  description: string;
  folderName: string;
  root: string;
  scope: "global" | "project" | "compatibility";
  enabled: boolean;
  trusted: boolean;
  requiredTools: string[];
};

export type SkillState = {
  skills: SkillRecord[];
  lastError: string | null;
};

export type ProjectView = {
  id: string;
  name: string;
  root: string;
  detail: string;
  status: string;
  active: boolean;
  createdAtMs: number;
  updatedAtMs: number;
};

export type SessionView = {
  id: string;
  projectId: string;
  name: string;
  detail: string;
  effort: AgentEffort;
  status: string;
  active: boolean;
  archived: boolean;
  archivedAtMs: number | null;
  createdAtMs: number;
  updatedAtMs: number;
};

export type ProjectSessionState = {
  projects: ProjectView[];
  sessions: SessionView[];
  activeProjectId: string;
  activeSessionId: string;
  lastError: string | null;
};

export type AgentAttachment = {
  id: string;
  name: string;
  path: string;
  mimeType: string;
  sizeBytes: number;
};

export type QueuedAgentMessage = {
  id: string;
  sessionId: string;
  prompt: string;
  attachments: AgentAttachment[];
  effort: AgentEffort;
  mode: "queue" | "steer";
  createdAtMs: number;
  updatedAtMs: number;
};

export type AttachmentUpload = {
  name: string;
  mimeType: string;
  dataBase64: string;
};

export type ArtifactPreview = {
  kind: "image" | "html" | "markdown" | "text" | "file";
  mimeType: string;
  content: string | null;
  dataUrl: string | null;
  sizeBytes: number;
};

export type TimelineEntry = {
  sequence?: number;
  label: string;
  detail: string;
  kind: "message" | "tool" | "permission" | "model";
  state: "done" | "pending" | "idle";
  timestampMs: number;
  workflowProgress?: {
    completedSteps: number;
    totalSteps: number;
    currentStepId: string | null;
    stepStatus: string | null;
    continuations: number;
    recoverable: boolean;
  };
};

export type PermissionAudit = {
  id: string;
  risk: string;
  action: string;
  reason: string;
  scope: string;
  status: "pending" | "resolved";
  decision: "allow_once" | "allow_for_session" | "deny" | null;
  requestedAtMs: number;
  resolvedAtMs: number | null;
};

export type Phase3State = {
  timeline: TimelineEntry[];
  permissions: PermissionAudit[];
};

export type PermissionReviewItem = {
  requestId: string;
  action: string;
  risk: string;
  reason: string;
  scope: string;
  source: "agent" | "tool" | "browser" | "test";
  projectId: string | null;
  projectName: string | null;
  sessionId: string | null;
  sessionName: string | null;
  input: string;
  requestedAtMs: number;
  canAllowSession: boolean;
};

export type PermissionReviewState = {
  pending: PermissionReviewItem[];
};

export type ProviderConfigState = {
  baseUrl: string;
  model: string;
  conductorModel: string;
  plannerModel: string;
  executorModel: string;
  reviewerModel: string;
  summarizerModel: string;
  embeddingModel: string;
  imageModel: string;
  imageEndpoint: string;
  collaborationPolicy: string;
  promptEvolutionEnabled: boolean;
  contextWindowTokens: number;
  agentSystemPrompt: string;
  apiKeySet: boolean;
};

export type ProviderConfigInput = {
  baseUrl: string;
  apiKey: string;
  model: string;
  conductorModel: string;
  plannerModel: string;
  executorModel: string;
  reviewerModel: string;
  summarizerModel: string;
  embeddingModel: string;
  imageModel: string;
  imageEndpoint: string;
  collaborationPolicy: string;
  promptEvolutionEnabled: boolean;
  contextWindowTokens: number;
  agentSystemPrompt: string;
};

export type ProviderModelsState = {
  models: string[];
  fetchedAtMs: number;
  lastError: string | null;
};

export type ImageEndpointValidationState = {
  endpoint: string;
  valid: boolean;
  lastError: string | null;
};

export type ChatMessageView = {
  sequence?: number;
  role: "user" | "assistant" | "system" | "tool" | "reviewer";
  content: string;
  timestampMs: number;
  attachments?: AgentAttachment[];
};

export type Phase4State = {
  provider: ProviderConfigState;
  promptEvolution: PromptEvolutionState;
  timeline: TimelineEntry[];
  messages: ChatMessageView[];
  lastError: string | null;
};

export type PromptEvolutionProfileState = {
  id: string;
  effort: string;
  generation: number;
  runs: number;
  trainRuns: number;
  holdoutRuns: number;
  successRate: number;
  averageReward: number | null;
  averageRelativeReward: number | null;
  averageStepCredit: number | null;
  averageQuality: number | null;
  averageLatencyMs: number;
  averageTokens: number;
  frontier: boolean;
  champion: boolean;
  learned: boolean;
  next: boolean;
};

export type PromptEvolutionEffortState = {
  effort: string;
  status: string;
  championId: string | null;
  championScore: number | null;
  stagnantGenerations: number;
  evaluatedGenerations: number;
  freezeReason: string | null;
  shadowRatePercent: number;
  nextMode: string;
  pairedRuns: number;
  replayRuns: number;
  readyProfiles: number;
  evaluationInflight: boolean;
  stableProfileId: string;
  canaryProfileId: string | null;
  canaryPercent: number;
  promotionConfidence: number | null;
  rollbackCount: number;
  rolloutStatus: string;
};

export type PromptEvolutionState = {
  enabled: boolean;
  observedRuns: number;
  generation: number;
  populationSize: number;
  frontierProfiles: number;
  pairedRuns: number;
  replayRuns: number;
  evaluationInflight: boolean;
  efforts: PromptEvolutionEffortState[];
  profiles: PromptEvolutionProfileState[];
};

export type ToolSpecView = {
  name: string;
  description: string;
  risk: string;
  inputSchema: string;
};

export type ToolRunView = {
  invocationId: string;
  toolName: string;
  status: string;
  output: string;
  timestampMs: number;
};

export type ToolApprovalView = {
  requestId: string;
  invocationId: string;
  toolName: string;
  risk: string;
  reason: string;
  scope: string;
  input: string;
  requestedAtMs: number;
};

export type Phase5State = {
  timeline: TimelineEntry[];
  tools: ToolSpecView[];
  pendingApprovals: ToolApprovalView[];
  results: ToolRunView[];
  lastError: string | null;
};

export type OrchestrationStepView = {
  orchestrationId: string;
  policy: string;
  stepIndex: number;
  role: string;
  model: string;
  output: string;
  latencyMs: number | null;
  timestampMs: number;
};

export type Phase6State = {
  timeline: TimelineEntry[];
  steps: OrchestrationStepView[];
  lastError: string | null;
};

export type RagStatsView = {
  filesIndexed: number;
  chunksIndexed: number;
  indexedAtMs: number;
};

export type MemoryStatsView = {
  records: number;
  requirements: number;
  outcomes: number;
  evidence: number;
  recalls: number;
  updatedAtMs: number;
};

export type RagSourceView = {
  path: string;
  startLine: number;
  endLine: number;
  fileHash: string;
  score: number;
  reason: string;
  text: string;
};

export type RetrievalChannelView = {
  name: string;
  resultCount: number;
  durationMs: number;
  topSources: string[];
  error: string | null;
};

export type RetrievalTraceView = {
  query: string;
  mode: string;
  channels: RetrievalChannelView[];
  selectedCount: number;
  durationMs: number;
  indexCacheHit: boolean;
  indexDurationMs: number;
};

export type GraphNodeView = {
  id: string;
  kind: string;
  label: string;
  sourcePath: string;
  focused: boolean;
};

export type GraphEdgeView = {
  id: string;
  from: string;
  to: string;
  kind: string;
};

export type GraphStateView = {
  totalNodes: number;
  totalEdges: number;
  nodes: GraphNodeView[];
  edges: GraphEdgeView[];
};

export type Phase7State = {
  timeline: TimelineEntry[];
  stats: RagStatsView;
  memory: MemoryStatsView;
  sources: RagSourceView[];
  retrievalTrace: RetrievalTraceView | null;
  graph: GraphStateView;
  answer: string | null;
  lastError: string | null;
};

export type BrowserObservationView = {
  invocationId: string;
  toolName: string;
  status: string;
  url: string | null;
  output: string;
  artifactPath: string | null;
  textPath: string | null;
  captureKind: string | null;
  timestampMs: number;
};

export type Phase8State = {
  timeline: TimelineEntry[];
  pendingApprovals: ToolApprovalView[];
  observations: BrowserObservationView[];
  lastError: string | null;
};

export type ContextCheckpointView = {
  id: string;
  generatedAtMs: number;
  eventCount: number;
  taskCount: number;
  latestEventMs: number;
  currentGoal: string | null;
  completedSteps: string[];
  pendingSteps: string[];
  decisions: string[];
  fileChanges: string[];
  commandsRun: string[];
  toolResults: string[];
  retrievals: string[];
  artifacts: string[];
  errors: string[];
  nextActions: string[];
  path: string | null;
  restorePack: string;
};

export type ContextState = {
  timeline: TimelineEntry[];
  checkpoint: ContextCheckpointView | null;
  lastError: string | null;
};

export type AgentState = {
  taskId: string;
  projectId: string | null;
  projectName: string | null;
  sessionId: string | null;
  sessionName: string | null;
  status: "idle" | "running" | "waiting_for_permission" | "completed" | "failed" | "cancelled";
  turnCount: number;
  maxTurns: number;
  transcriptMessages: number;
  contextTokensUsed: number;
  contextWindowTokens: number;
  contextRemainingPercent: number;
  contextUsageEstimated: boolean;
  runStartedAtMs: number;
  runBudgetMs: number;
  runModelCallBudget: number;
  runToolCallBudget: number;
  canCancel: boolean;
  canRetry: boolean;
  canContinue: boolean;
  eventCount: number;
  latestSequence: number;
  oldestSequence: number;
  hasOlderHistory: boolean;
  timeline: TimelineEntry[];
  messages: ChatMessageView[];
  pendingApprovals: ToolApprovalView[];
  queuedMessages: QueuedAgentMessage[];
  latestAnswer: string | null;
  lastError: string | null;
};

export type AgentStateRevision = {
  sessionId: string;
  eventCount: number;
  latestSequence: number;
  latestTimestampMs: number;
};

export type AgentStateDelta = {
  reset: boolean;
  latestSequence: number;
  state: AgentState;
};

export type AgentHistoryPage = {
  sessionId: string;
  oldestSequence: number;
  hasOlderHistory: boolean;
  timeline: TimelineEntry[];
  messages: ChatMessageView[];
};

export type AgentTraceStepView = {
  id: string;
  parentId: string | null;
  turnIndex: number;
  sequence: number;
  kind: "status" | "message" | "model" | "tool" | "permission" | "retrieval" | "error";
  label: string;
  status: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  latencyMs: number | null;
  model: string | null;
  toolName: string | null;
  requestId: string | null;
  toolCallId: string | null;
  permissionId: string | null;
  inputPreview: string | null;
  outputPreview: string | null;
  artifactPath: string | null;
  detail: string;
  metadata: Record<string, string>;
};

export type AgentTraceTurnView = {
  index: number;
  label: string;
  status: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  durationMs: number | null;
  steps: AgentTraceStepView[];
};

export type AgentTraceRoleSummary = {
  role: string;
  models: string[];
  calls: number;
  completed: number;
  degraded: number;
  latencyMs: number;
  firstTokenLatencyMs: number | null;
  totalTokens: number;
  evidenceCount: number;
};

export type AgentTraceState = {
  taskId: string;
  traceId: string;
  runId: string;
  projectId: string | null;
  projectName: string | null;
  sessionId: string | null;
  sessionName: string | null;
  status: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  durationMs: number | null;
  turnCount: number;
  stepCount: number;
  toolCallCount: number;
  permissionWaitCount: number;
  errorCount: number;
  roleSummaries: AgentTraceRoleSummary[];
  exportPath: string | null;
  turns: AgentTraceTurnView[];
  lastError: string | null;
};

export type AgentOutputArtifactView = {
  id: string;
  path: string;
  sourcePath: string | null;
  toolName: string;
  status: string;
  timestampMs: number;
  runId: string | null;
  version: number;
};

export type ModelStreamDelta = {
  taskId: string;
  requestId: string;
  sessionId: string | null;
  delta: string;
  done: boolean;
  reset: boolean;
  error: string | null;
};

let browserPhase3State: Phase3State = {
  timeline: [],
  permissions: []
};

let browserSidecarState: SidecarState = {
  browser: {
    path: "scripts/sidecars/browser-sidecar.js",
    exists: true,
    executable: true,
    healthy: true,
    healthOutput: "browser preview",
    envKey: "CINDX_BROWSER_SIDECAR"
  },
  computer: {
    path: "scripts/sidecars/computer-sidecar.js",
    exists: true,
    executable: true,
    healthy: true,
    healthOutput: "browser preview",
    envKey: "CINDX_COMPUTER_SIDECAR"
  },
  autoConfigure: true,
  lastError: null
};

let browserWebSearchConfig: WebSearchConfigState = {
  endpoint: "",
  apiKeySet: false,
  configured: false
};

let browserProjectSessionState: ProjectSessionState = {
  projects: [
    {
      id: "project-cindx",
      name: "Cindx",
      root: ".",
      detail: "current workspace",
      status: "Selected",
      active: true,
      createdAtMs: 0,
      updatedAtMs: 0
    }
  ],
  sessions: [
    {
      id: "session-runtime",
      projectId: "project-cindx",
      name: "Runtime Session",
      detail: "timeline + chat",
      effort: "auto",
      status: "Active",
      active: true,
      archived: false,
      archivedAtMs: null,
      createdAtMs: 0,
      updatedAtMs: 0
    }
  ],
  activeProjectId: "project-cindx",
  activeSessionId: "session-runtime",
  lastError: null
};

let browserPhase4State: Phase4State = {
  provider: {
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4.1-mini",
    conductorModel: "gpt-4.1-mini",
    plannerModel: "gpt-4.1-mini",
    executorModel: "gpt-4.1-mini",
    reviewerModel: "gpt-4.1-mini",
    summarizerModel: "gpt-4.1-mini",
    embeddingModel: "text-embedding-3-small",
    imageModel: "",
    imageEndpoint: "",
    collaborationPolicy: "auto_router",
    promptEvolutionEnabled: true,
    contextWindowTokens: 128000,
    agentSystemPrompt:
      "You are Cindx, a desktop-first assistant. Work carefully, be direct, and ask for clarification when the task is ambiguous.",
    apiKeySet: false
  },
  promptEvolution: {
    enabled: true,
    observedRuns: 0,
    generation: 1,
    populationSize: 15,
    frontierProfiles: 0,
    pairedRuns: 0,
    replayRuns: 0,
    evaluationInflight: false,
    efforts: ["fast", "auto", "pro"].map((effort) => ({
      effort,
      status: "exploring",
      championId: null,
      championScore: null,
      stagnantGenerations: 0,
      evaluatedGenerations: 0,
      freezeReason: null,
      shadowRatePercent: 10,
      nextMode: "explore",
      pairedRuns: 0,
      replayRuns: 0,
      readyProfiles: 0,
      evaluationInflight: false,
      stableProfileId: `seed-${effort}-v1`,
      canaryProfileId: null,
      canaryPercent: 0,
      promotionConfidence: null,
      rollbackCount: 0,
      rolloutStatus: "stable"
    })),
    profiles: ["fast", "auto", "pro"].map((effort) => ({
      id: `seed-${effort}-v1`,
      effort,
      generation: 0,
      runs: 0,
      trainRuns: 0,
      holdoutRuns: 0,
      successRate: 0,
      averageReward: null,
      averageRelativeReward: null,
      averageStepCredit: null,
      averageQuality: null,
      averageLatencyMs: 0,
      averageTokens: 0,
      frontier: false,
      champion: false,
      learned: false,
      next: true
    }))
  },
  timeline: [],
  messages: [],
  lastError: null
};

let browserPhase5State: Phase5State = {
  timeline: [],
  tools: [
    {
      name: "file.read",
      description: "Read a UTF-8 file inside the workspace.",
      risk: "read_only",
      inputSchema: "path=README.md"
    },
    {
      name: "file.list",
      description: "List files and directories inside the workspace.",
      risk: "read_only",
      inputSchema: "path=."
    },
    {
      name: "file.search",
      description: "Search UTF-8 files inside the workspace for a literal query.",
      risk: "read_only",
      inputSchema: "path=.\nquery=Phase"
    },
    {
      name: "file.write",
      description: "Write UTF-8 content to a file inside the workspace.",
      risk: "writes_workspace",
      inputSchema: "path=.cindx/demo.txt\ncontent=hello"
    },
    {
      name: "shell.run",
      description: "Run a shell command in the workspace.",
      risk: "executes_process",
      inputSchema: "command=pwd"
    },
    {
      name: "web.search",
      description: "Search the web through a lightweight HTML endpoint.",
      risk: "uses_network",
      inputSchema: "query=local agent"
    },
    {
      name: "browser.open",
      description: "Open a URL in a reusable CDP browser session.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.extract_text",
      description: "Read dynamic page text and its accessibility snapshot.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.capture",
      description: "Capture a webpage observation artifact.",
      risk: "uses_network",
      inputSchema: "url=https://example.com"
    },
    {
      name: "browser.click",
      description: "Click a selector or coordinate through the browser controller.",
      risk: "uses_network",
      inputSchema: "selector=button.primary"
    },
    {
      name: "browser.type",
      description: "Type text through the browser controller.",
      risk: "sensitive_context",
      inputSchema: "selector=#q\ntext=hello"
    },
    {
      name: "browser.scroll",
      description: "Scroll the current browser page.",
      risk: "uses_network",
      inputSchema: "delta_y=600"
    },
    {
      name: "browser.tabs",
      description: "List tabs in the reusable browser session.",
      risk: "uses_network",
      inputSchema: ""
    },
    {
      name: "browser.select_tab",
      description: "Select a tab by its CDP target id.",
      risk: "uses_network",
      inputSchema: "tab_id=<target-id>"
    },
    {
      name: "computer.screenshot",
      description: "Capture a desktop screenshot artifact with redaction metadata.",
      risk: "sensitive_context",
      inputSchema: "redaction=manual"
    },
    {
      name: "computer.click",
      description: "Click a local desktop coordinate through the controller.",
      risk: "sensitive_context",
      inputSchema: "x=120\ny=240"
    },
    {
      name: "computer.type",
      description: "Type text through the controller.",
      risk: "sensitive_context",
      inputSchema: "text=hello"
    },
    {
      name: "computer.key",
      description: "Press a keyboard shortcut through the controller.",
      risk: "destructive",
      inputSchema: "key=Cmd+S\ndestructive=false"
    },
    {
      name: "computer.scroll",
      description: "Scroll through the controller.",
      risk: "sensitive_context",
      inputSchema: "delta_y=600"
    }
  ],
  pendingApprovals: [],
  results: [],
  lastError: null
};

let browserPhase6State: Phase6State = {
  timeline: [],
  steps: [],
  lastError: null
};

let browserPhase7State: Phase7State = {
  timeline: [],
  stats: {
    filesIndexed: 0,
    chunksIndexed: 0,
    indexedAtMs: 0
  },
  memory: {
    records: 0,
    requirements: 0,
    outcomes: 0,
    evidence: 0,
    recalls: 0,
    updatedAtMs: 0
  },
  sources: [],
  retrievalTrace: null,
  graph: {
    totalNodes: 0,
    totalEdges: 0,
    nodes: [],
    edges: []
  },
  answer: null,
  lastError: null
};

let browserPhase8State: Phase8State = {
  timeline: [],
  pendingApprovals: [],
  observations: [],
  lastError: null
};

let browserContextState: ContextState = {
  timeline: [],
  checkpoint: createBrowserContextCheckpoint(null),
  lastError: null
};

let browserAgentState: AgentState = {
  taskId: "phase-16-agent-loop",
  projectId: "project-cindx",
  projectName: "Cindx",
  sessionId: "session-runtime",
  sessionName: "Runtime Session",
  status: "idle",
  turnCount: 0,
  maxTurns: 24,
  transcriptMessages: 0,
  contextTokensUsed: 0,
  contextWindowTokens: 128000,
  contextRemainingPercent: 100,
  contextUsageEstimated: true,
  runStartedAtMs: 0,
  runBudgetMs: 0,
  runModelCallBudget: 0,
  runToolCallBudget: 0,
  canCancel: false,
  canRetry: false,
  canContinue: false,
  eventCount: 0,
  latestSequence: 0,
  oldestSequence: 0,
  hasOlderHistory: false,
  timeline: [],
  messages: [],
  pendingApprovals: [],
  queuedMessages: [],
  latestAnswer: null,
  lastError: null
};

let browserAgentTraceState: AgentTraceState = {
  taskId: "phase-16-agent-loop",
  traceId: "browser-preview",
  runId: "browser-preview",
  projectId: "project-cindx",
  projectName: "Cindx",
  sessionId: "session-runtime",
  sessionName: "Runtime Session",
  status: "idle",
  startedAtMs: 0,
  finishedAtMs: null,
  durationMs: null,
  turnCount: 0,
  stepCount: 0,
  toolCallCount: 0,
  permissionWaitCount: 0,
  errorCount: 0,
  roleSummaries: [],
  exportPath: null,
  turns: [],
  lastError: null
};

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function revealMainWindow(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("reveal_main_window");
}

export async function setSidebarMaterialWidth(width: number): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("set_sidebar_material_width", { width });
}

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  try {
    return await invoke<RuntimeStatus>("get_runtime_status");
  } catch {
    return {
      appVersion: DESKTOP_VERSION,
      kernelStatus: "browser preview",
      workspaceRoot: ".",
      orchestrationModes: ["single", "plan_execute_review", "best_of_n", "auto_router"],
      registeredTools: [
        "file.read",
        "file.list",
        "file.write",
        "file.search",
        "shell.run",
        "rag.index",
        "rag.search",
        "rag.answer",
        "web.search",
        "browser.open",
        "browser.extract_text",
        "browser.capture",
        "browser.click",
        "browser.type",
        "browser.scroll",
        "browser.tabs",
        "browser.select_tab",
        "computer.screenshot",
        "computer.click",
        "computer.type",
        "computer.key",
        "computer.scroll"
      ]
    };
  }
}

export async function saveWorkspaceRoot(path: string): Promise<RuntimeStatus> {
  try {
    return await invoke<RuntimeStatus>("save_workspace_root", { input: { path } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return {
      appVersion: DESKTOP_VERSION,
      kernelStatus: "browser preview",
      workspaceRoot: path,
      orchestrationModes: ["single", "plan_execute_review", "best_of_n", "auto_router"],
      registeredTools: [
        "file.read",
        "file.list",
        "file.write",
        "file.search",
        "shell.run",
        "rag.index",
        "rag.search",
        "rag.answer",
        "web.search",
        "browser.open",
        "browser.extract_text",
        "browser.capture",
        "browser.click",
        "browser.type",
        "browser.scroll",
        "browser.tabs",
        "browser.select_tab",
        "computer.screenshot",
        "computer.click",
        "computer.type",
        "computer.key",
        "computer.scroll"
      ]
    };
  }
}

export async function pickWorkspaceFolder(
  initialPath?: string
): Promise<string | null> {
  try {
    return await invoke<string | null>("pick_workspace_folder", { initialPath });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return initialPath ?? null;
  }
}

export async function getSidecarState(): Promise<SidecarState> {
  try {
    return await invoke<SidecarState>("get_sidecar_state");
  } catch {
    return browserSidecarState;
  }
}

export async function getWebSearchConfig(): Promise<WebSearchConfigState> {
  try {
    return await invoke<WebSearchConfigState>("get_web_search_config");
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserWebSearchConfig;
  }
}

export async function saveWebSearchConfig(
  input: WebSearchConfigInput
): Promise<WebSearchConfigState> {
  try {
    return await invoke<WebSearchConfigState>("save_web_search_config", { input });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserWebSearchConfig = {
      endpoint: input.endpoint,
      apiKeySet: Boolean(input.apiKey),
      configured: Boolean(input.endpoint)
    };
    return browserWebSearchConfig;
  }
}

export async function getMcpState(): Promise<McpState> {
  try {
    return await invoke<McpState>("get_mcp_state");
  } catch (error) {
    return { servers: [], lastError: String(error) };
  }
}

export async function saveMcpServers(servers: McpServerConfig[]): Promise<McpState> {
  return await invoke<McpState>("save_mcp_servers", { input: { servers } });
}

export async function upsertMcpServer(server: McpServerConfig): Promise<McpState> {
  return await invoke<McpState>("upsert_mcp_server", { input: { server } });
}

export async function updateMcpServerPolicy(
  serverId: string,
  enabled: boolean,
  requireApproval: boolean
): Promise<McpState> {
  return await invoke<McpState>("update_mcp_server_policy", {
    input: { serverId, enabled, requireApproval }
  });
}

export async function removeMcpServer(serverId: string): Promise<McpState> {
  return await invoke<McpState>("remove_mcp_server", { serverId });
}

export async function refreshMcpServer(serverId: string): Promise<McpState> {
  return await invoke<McpState>("refresh_mcp_server", { serverId });
}

export async function getSkillState(): Promise<SkillState> {
  try {
    return await invoke<SkillState>("get_skill_state");
  } catch (error) {
    return { skills: [], lastError: String(error) };
  }
}

export async function refreshSkills(): Promise<SkillState> {
  return await invoke<SkillState>("refresh_skills");
}

export async function saveSkillPreference(
  skillId: string,
  enabled: boolean,
  trusted: boolean
): Promise<SkillState> {
  return await invoke<SkillState>("save_skill_preference", {
    input: { skillId, enabled, trusted }
  });
}

export async function installSkillPackage(dataBase64: string): Promise<SkillState> {
  return await invoke<SkillState>("install_skill_package", { input: { dataBase64 } });
}

export async function installSkillUrl(url: string): Promise<SkillState> {
  return await invoke<SkillState>("install_skill_url", { input: { url } });
}

export async function saveSidecarConfig(input: SidecarConfigInput): Promise<SidecarState> {
  try {
    return await invoke<SidecarState>("save_sidecar_config", { input });
  } catch {
    browserSidecarState = {
      browser: {
        ...browserSidecarState.browser,
        path: input.browserPath,
        healthOutput: "Browser preview cannot run sidecar health checks."
      },
      computer: {
        ...browserSidecarState.computer,
        path: input.computerPath,
        healthOutput: "Browser preview cannot run sidecar health checks."
      },
      autoConfigure: input.autoConfigure,
      lastError: null
    };
    return browserSidecarState;
  }
}

function withProjectSessionSelection(
  state: ProjectSessionState,
  activeProjectId: string,
  activeSessionId: string
): ProjectSessionState {
  const next = {
    ...state,
    activeProjectId,
    activeSessionId,
    projects: state.projects.map((project) => ({
      ...project,
      active: project.id === activeProjectId,
      status: project.id === activeProjectId ? "Selected" : "Ready"
    })),
    sessions: state.sessions.map((session) => ({
      ...session,
      active: session.id === activeSessionId,
      status:
        session.id === activeSessionId ? "Active" : session.archived ? "Archived" : "Ready"
    })),
    lastError: null
  };
  const activeProject = next.projects.find((project) => project.active) ?? null;
  const activeSession = next.sessions.find((session) => session.active) ?? null;
  browserAgentState = {
    ...browserAgentState,
    projectId: activeProject?.id ?? null,
    projectName: activeProject?.name ?? null,
    sessionId: activeSession?.id ?? null,
    sessionName: activeSession?.name ?? null
  };
  browserAgentTraceState = {
    ...browserAgentTraceState,
    projectId: activeProject?.id ?? null,
    projectName: activeProject?.name ?? null,
    sessionId: activeSession?.id ?? null,
    sessionName: activeSession?.name ?? null
  };
  return next;
}

export async function getProjectSessionState(): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("get_project_session_state");
  } catch {
    return browserProjectSessionState;
  }
}

export async function createProject(name: string, root: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("create_project", { input: { name, root } });
  } catch {
    const now = Date.now();
    const suffix = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "project";
    const projectId = `project-${suffix}-${now}`;
    const sessionId = `session-${suffix}-${now}`;
    browserProjectSessionState = withProjectSessionSelection(
      {
        ...browserProjectSessionState,
        projects: [
          ...browserProjectSessionState.projects,
          {
            id: projectId,
            name,
            root,
            detail: "workspace project",
            status: "Ready",
            active: false,
            createdAtMs: now,
            updatedAtMs: now
          }
        ],
        sessions: [
          ...browserProjectSessionState.sessions,
          {
            id: sessionId,
            projectId,
            name: `${name} Session`,
            detail: "timeline + chat",
            effort: "auto",
            status: "Ready",
            active: false,
            archived: false,
            archivedAtMs: null,
            createdAtMs: now,
            updatedAtMs: now
          }
        ]
      },
      projectId,
      sessionId
    );
    return browserProjectSessionState;
  }
}

export async function createSession(
  name: string,
  projectId?: string
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("create_session", {
      input: { name, projectId: projectId ?? null }
    });
  } catch {
    const now = Date.now();
    const activeProjectId = projectId ?? browserProjectSessionState.activeProjectId;
    const sessionId = `session-${now}`;
    browserProjectSessionState = withProjectSessionSelection(
      {
        ...browserProjectSessionState,
        sessions: [
          ...browserProjectSessionState.sessions,
          {
            id: sessionId,
            projectId: activeProjectId,
            name,
            detail: "timeline + chat",
            effort: "auto",
            status: "Ready",
            active: false,
            archived: false,
            archivedAtMs: null,
            createdAtMs: now,
            updatedAtMs: now
          }
        ]
      },
      activeProjectId,
      sessionId
    );
    return browserProjectSessionState;
  }
}

export async function renameProject(
  projectId: string,
  name: string
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("rename_project", {
      input: { projectId, name }
    });
  } catch {
    const normalizedName = name.trim().replace(/[\n\r]/g, "");
    if (!normalizedName) return browserProjectSessionState;
    const now = Date.now();
    browserProjectSessionState = {
      ...browserProjectSessionState,
      projects: browserProjectSessionState.projects.map((project) =>
        project.id === projectId
          ? { ...project, name: normalizedName, updatedAtMs: now }
          : project
      )
    };
    return browserProjectSessionState;
  }
}

export async function deleteProject(projectId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("delete_project", { input: { projectId } });
  } catch {
    const projectIndex = browserProjectSessionState.projects.findIndex(
      (project) => project.id === projectId
    );
    if (projectIndex < 0) return browserProjectSessionState;
    const projects = browserProjectSessionState.projects.filter(
      (project) => project.id !== projectId
    );
    const sessions = browserProjectSessionState.sessions.filter(
      (session) => session.projectId !== projectId
    );
    const next = { ...browserProjectSessionState, projects, sessions };
    if (browserProjectSessionState.activeProjectId !== projectId) {
      browserProjectSessionState = withProjectSessionSelection(
        next,
        browserProjectSessionState.activeProjectId,
        browserProjectSessionState.activeSessionId
      );
    } else if (projects.length === 0) {
      browserProjectSessionState = withProjectSessionSelection(next, "", "");
    } else {
      const nextProject = projects[Math.min(projectIndex, projects.length - 1)];
      browserProjectSessionState = ensureBrowserOpenSession(next, nextProject.id);
    }
    return browserProjectSessionState;
  }
}

export async function forkSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("fork_session", { input: { sessionId } });
  } catch {
    const source = browserProjectSessionState.sessions.find((session) => session.id === sessionId);
    if (!source) return browserProjectSessionState;
    const now = Date.now();
    const fork = {
      ...source,
      id: `session-fork-${now}`,
      name: `${source.name} Fork`,
      detail: `Fork of ${source.name}`,
      effort: "auto" as AgentEffort,
      status: "Ready",
      active: false,
      archived: false,
      archivedAtMs: null,
      createdAtMs: now,
      updatedAtMs: now
    };
    browserProjectSessionState = withProjectSessionSelection(
      {
        ...browserProjectSessionState,
        sessions: [...browserProjectSessionState.sessions, fork]
      },
      source.projectId,
      fork.id
    );
    return browserProjectSessionState;
  }
}

export async function renameSession(
  sessionId: string,
  name: string
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("rename_session", {
      input: { sessionId, name }
    });
  } catch {
    const normalizedName = name.trim().replace(/[\n\r]/g, "");
    const source = browserProjectSessionState.sessions.find(
      (session) => session.id === sessionId
    );
    if (!normalizedName || !source) return browserProjectSessionState;
    const now = Date.now();
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId
          ? { ...session, name: normalizedName, updatedAtMs: now }
          : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function setSessionEffort(
  sessionId: string,
  effort: AgentEffort
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("set_session_effort", {
      input: { sessionId, effort }
    });
  } catch {
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId ? { ...session, effort } : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function generateSessionTitle(
  sessionId: string,
  prompt: string,
  answer: string
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("generate_session_title", {
      input: { sessionId, prompt, answer }
    });
  } catch {
    return await getProjectSessionState();
  }
}

export async function stageAgentAttachments(
  sessionId: string,
  files: AttachmentUpload[]
): Promise<AgentAttachment[]> {
  return invoke<AgentAttachment[]>("stage_agent_attachments", {
    input: { sessionId, files }
  });
}

export async function removeAgentAttachment(sessionId: string, path: string): Promise<void> {
  await invoke<void>("remove_agent_attachment", { input: { sessionId, path } });
}

export async function archiveSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("archive_session", { input: { sessionId } });
  } catch {
    const source = browserProjectSessionState.sessions.find((session) => session.id === sessionId);
    if (!source) return browserProjectSessionState;
    const now = Date.now();
    let next = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId
          ? { ...session, active: false, archived: true, archivedAtMs: now, status: "Archived" }
          : session
      )
    };
    if (browserProjectSessionState.activeSessionId === sessionId) {
      next = ensureBrowserOpenSession(next, source.projectId);
    }
    browserProjectSessionState = next;
    return next;
  }
}

export async function restoreSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("restore_session", { input: { sessionId } });
  } catch {
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId
          ? { ...session, archived: false, archivedAtMs: null, status: "Ready" }
          : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function deleteSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("delete_session", { input: { sessionId } });
  } catch {
    const source = browserProjectSessionState.sessions.find((session) => session.id === sessionId);
    if (!source) return browserProjectSessionState;
    let next = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.filter((session) => session.id !== sessionId)
    };
    if (browserProjectSessionState.activeSessionId === sessionId) {
      next = ensureBrowserOpenSession(next, source.projectId);
    }
    browserProjectSessionState = next;
    return next;
  }
}

function ensureBrowserOpenSession(
  state: ProjectSessionState,
  projectId: string
): ProjectSessionState {
  const available = state.sessions.find(
    (session) => session.projectId === projectId && !session.archived
  );
  if (available) return withProjectSessionSelection(state, projectId, available.id);
  const now = Date.now();
  const session: SessionView = {
    id: `session-${now}`,
    projectId,
    name: "New Session",
    detail: "timeline + chat",
    effort: "auto",
    status: "Ready",
    active: false,
    archived: false,
    archivedAtMs: null,
    createdAtMs: now,
    updatedAtMs: now
  };
  return withProjectSessionSelection(
    { ...state, sessions: [...state.sessions, session] },
    projectId,
    session.id
  );
}

export async function selectProject(projectId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("select_project", { input: { projectId } });
  } catch {
    const nextSession =
      browserProjectSessionState.sessions.find(
        (session) => session.projectId === projectId && !session.archived
      )?.id ??
      browserProjectSessionState.activeSessionId;
    browserProjectSessionState = withProjectSessionSelection(
      browserProjectSessionState,
      projectId,
      nextSession
    );
    return browserProjectSessionState;
  }
}

export async function selectSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("select_session", { input: { sessionId } });
  } catch {
    const session = browserProjectSessionState.sessions.find((item) => item.id === sessionId);
    browserProjectSessionState = withProjectSessionSelection(
      browserProjectSessionState,
      session?.projectId ?? browserProjectSessionState.activeProjectId,
      sessionId
    );
    return browserProjectSessionState;
  }
}

export async function getPhase3State(): Promise<Phase3State> {
  try {
    return await invoke<Phase3State>("get_phase3_state");
  } catch {
    return browserPhase3State;
  }
}

export async function getPermissionReviewState(): Promise<PermissionReviewState> {
  try {
    return await invoke<PermissionReviewState>("get_permission_review_state");
  } catch {
    const activeSession = browserProjectSessionState.sessions.find((session) => session.active);
    const activeProject = browserProjectSessionState.projects.find((project) => project.active);
    const context = {
      projectId: activeProject?.id ?? null,
      projectName: activeProject?.name ?? null,
      sessionId: activeSession?.id ?? null,
      sessionName: activeSession?.name ?? null
    };
    const toolReviews = browserPhase5State.pendingApprovals.map((approval) => ({
      requestId: approval.requestId,
      action: approval.toolName,
      risk: approval.risk,
      reason: approval.reason,
      scope: approval.scope,
      source: "tool" as const,
      ...context,
      input: approval.input,
      requestedAtMs: approval.requestedAtMs,
      canAllowSession: false
    }));
    const browserReviews = browserPhase8State.pendingApprovals.map((approval) => ({
      requestId: approval.requestId,
      action: approval.toolName,
      risk: approval.risk,
      reason: approval.reason,
      scope: approval.scope,
      source: "browser" as const,
      ...context,
      input: approval.input,
      requestedAtMs: approval.requestedAtMs,
      canAllowSession: false
    }));
    const agentReviews = browserAgentState.pendingApprovals.map((approval) => ({
      requestId: approval.requestId,
      action: approval.toolName,
      risk: approval.risk,
      reason: approval.reason,
      scope: approval.scope,
      source: "agent" as const,
      ...context,
      input: approval.input,
      requestedAtMs: approval.requestedAtMs,
      canAllowSession: approval.risk !== "destructive"
    }));
    const testReviews = browserPhase3State.permissions
      .filter((permission) => permission.status === "pending")
      .map((permission) => ({
        requestId: permission.id,
        action: permission.action,
        risk: permission.risk,
        reason: permission.reason,
        scope: permission.scope,
        source: "test" as const,
        projectId: null,
        projectName: null,
        sessionId: null,
        sessionName: null,
        input: "",
        requestedAtMs: permission.requestedAtMs,
        canAllowSession: false
      }));
    return {
      pending: [...agentReviews, ...toolReviews, ...browserReviews, ...testReviews].sort(
        (left, right) => right.requestedAtMs - left.requestedAtMs
      )
    };
  }
}

export async function requestMockPermission(): Promise<Phase3State> {
  try {
    return await invoke<Phase3State>("request_mock_permission");
  } catch {
    const now = Date.now();
    const id = `perm-browser-${now}`;
    browserPhase3State = {
      timeline: [
        ...browserPhase3State.timeline,
        {
          label: "Tool proposed",
          detail: "Mock shell.run proposed",
          kind: "tool",
          state: "pending",
          timestampMs: now
        },
        {
          label: "Permission requested",
          detail: "Permission requested for shell.run",
          kind: "permission",
          state: "pending",
          timestampMs: now
        }
      ],
      permissions: [
        {
          id,
          risk: "execute",
          action: "shell.run",
          reason: "Run a harmless mock command to verify the permission gate.",
          scope: ".",
          status: "pending",
          decision: null,
          requestedAtMs: now,
          resolvedAtMs: null
        },
        ...browserPhase3State.permissions
      ]
    };
    return browserPhase3State;
  }
}

export async function resolvePermission(
  requestId: string,
  decision: "allow_once" | "allow_for_session" | "deny"
): Promise<Phase3State> {
  try {
    return await invoke<Phase3State>("resolve_permission", { requestId, decision });
  } catch {
    const now = Date.now();
    browserPhase3State = {
      timeline: [
        ...browserPhase3State.timeline.map((entry) =>
          entry.state === "pending" ? { ...entry, state: "done" as const } : entry
        ),
        {
          label: "Permission resolved",
          detail: `Permission ${decision === "deny" ? "denied" : "approved"} with ${decision}`,
          kind: "permission",
          state: "done",
          timestampMs: now
        }
      ],
      permissions: browserPhase3State.permissions.map((permission) =>
        permission.id === requestId
          ? {
              ...permission,
              status: "resolved",
              decision,
              resolvedAtMs: now
            }
          : permission
      )
    };
    return browserPhase3State;
  }
}

export async function getPhase4State(): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("get_phase4_state");
  } catch {
    return browserPhase4State;
  }
}

export async function saveProviderConfig(input: ProviderConfigInput): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("save_provider_config", { input });
  } catch {
    browserPhase4State = {
      ...browserPhase4State,
      provider: {
        baseUrl: input.baseUrl,
        model: input.model,
        conductorModel: input.conductorModel || input.plannerModel || input.model,
        plannerModel: input.plannerModel || input.model,
        executorModel: input.executorModel || input.model,
        reviewerModel: input.reviewerModel || input.model,
        summarizerModel: input.summarizerModel || input.model,
        embeddingModel: input.embeddingModel || "text-embedding-3-small",
        imageModel: input.imageModel,
        imageEndpoint: input.imageEndpoint,
        collaborationPolicy: input.collaborationPolicy || "auto_router",
        promptEvolutionEnabled: input.promptEvolutionEnabled,
        contextWindowTokens: Math.max(4096, input.contextWindowTokens || 128000),
        agentSystemPrompt: input.agentSystemPrompt,
        apiKeySet: Boolean(input.apiKey) || browserPhase4State.provider.apiKeySet
      },
      timeline: [
        ...browserPhase4State.timeline,
        {
          label: "Status",
          detail: "Provider config saved",
          kind: "message",
          state: "done",
          timestampMs: Date.now()
        }
      ],
      lastError: null
    };
    return browserPhase4State;
  }
}

export async function setPromptEvolutionEnabled(enabled: boolean): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("set_prompt_evolution_enabled", { enabled });
  } catch {
    browserPhase4State = {
      ...browserPhase4State,
      provider: {
        ...browserPhase4State.provider,
        promptEvolutionEnabled: enabled
      },
      promptEvolution: {
        ...browserPhase4State.promptEvolution,
        enabled
      }
    };
    return browserPhase4State;
  }
}

export async function listProviderModels(
  input: Pick<ProviderConfigInput, "baseUrl" | "apiKey">
): Promise<ProviderModelsState> {
  try {
    return await invoke<ProviderModelsState>("list_provider_models", { input });
  } catch (error) {
    return {
      models: [],
      fetchedAtMs: 0,
      lastError: error instanceof Error ? error.message : String(error)
    };
  }
}

export async function validateImageEndpoint(
  input: Pick<
    ProviderConfigInput,
    "baseUrl" | "imageModel" | "imageEndpoint"
  >
): Promise<ImageEndpointValidationState> {
  try {
    return await invoke<ImageEndpointValidationState>("validate_image_endpoint", { input });
  } catch (error) {
    const endpoint = input.imageEndpoint.trim() || input.baseUrl.trim();
    let valid = false;
    try {
      const parsed = new URL(endpoint);
      valid = Boolean(input.imageModel.trim()) && ["http:", "https:"].includes(parsed.protocol);
    } catch {
      valid = false;
    }
    return {
      endpoint,
      valid,
      lastError: valid ? null : error instanceof Error ? error.message : String(error)
    };
  }
}

export async function sendModelPrompt(prompt: string): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("send_model_prompt", { prompt });
  } catch {
    const now = Date.now();
    const answer = "Browser preview cannot call the local Rust provider. Open the Tauri app to send this prompt.";
    browserPhase4State = {
      ...browserPhase4State,
      timeline: [
        ...browserPhase4State.timeline,
        {
          label: "Message",
          detail: `user: ${prompt}`,
          kind: "message",
          state: "done",
          timestampMs: now
        },
        {
          label: "Error",
          detail: answer,
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      messages: [
        ...browserPhase4State.messages,
        { role: "user", content: prompt, timestampMs: now }
      ],
      lastError: answer
    };
    return browserPhase4State;
  }
}

export async function getAgentState(sessionId?: string | null): Promise<AgentState> {
  try {
    return await invoke<AgentState>("get_agent_state", { sessionId: sessionId ?? null });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserAgentState;
  }
}

export async function getAgentStateRevision(sessionId: string): Promise<AgentStateRevision> {
  try {
    return await invoke<AgentStateRevision>("get_agent_state_revision", { sessionId });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const events = browserAgentState.timeline;
    return {
      sessionId,
      eventCount: events.length,
      latestSequence: events.length,
      latestTimestampMs: events[events.length - 1]?.timestampMs ?? 0
    };
  }
}

export async function getAgentStateDelta(
  sessionId: string,
  afterSequence: number
): Promise<AgentStateDelta> {
  try {
    return await invoke<AgentStateDelta>("get_agent_state_delta", {
      sessionId,
      afterSequence
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return {
      reset: true,
      latestSequence:
        browserAgentState.timeline[browserAgentState.timeline.length - 1]?.sequence ??
        browserAgentState.timeline.length,
      state: browserAgentState
    };
  }
}

export async function getAgentHistoryPage(
  sessionId: string,
  beforeSequence: number,
  limit = 360
): Promise<AgentHistoryPage> {
  try {
    return await invoke<AgentHistoryPage>("get_agent_history_page", {
      sessionId,
      beforeSequence,
      limit
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return {
      sessionId,
      oldestSequence: browserAgentState.oldestSequence,
      hasOlderHistory: false,
      timeline: [],
      messages: []
    };
  }
}

export async function getAgentTraceState(sessionId?: string | null): Promise<AgentTraceState> {
  try {
    return await invoke<AgentTraceState>("get_agent_trace_state", { sessionId: sessionId ?? null });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserAgentTraceState;
  }
}

export async function getAgentSessionOutputs(
  sessionId: string
): Promise<AgentOutputArtifactView[]> {
  try {
    return await invoke<AgentOutputArtifactView[]>("get_agent_session_outputs", { sessionId });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return [];
  }
}

export async function exportAgentTraceJsonl(sessionId?: string | null): Promise<AgentTraceState> {
  try {
    return await invoke<AgentTraceState>("export_agent_trace_jsonl", {
      sessionId: sessionId ?? null
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserAgentTraceState = {
      ...browserAgentTraceState,
      lastError: "Browser preview cannot export the Rust agent trace. Open the Tauri app to export JSONL."
    };
    return browserAgentTraceState;
  }
}

function currentAgentTimeContext() {
  const now = new Date();
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "local";
  const localTime = new Intl.DateTimeFormat(undefined, {
    dateStyle: "full",
    timeStyle: "long"
  }).format(now);
  return `${localTime} (${timeZone}; UTC ${now.toISOString()})`;
}

export async function runAgentTask(
  prompt: string,
  sessionId: string,
  attachments: AgentAttachment[] = [],
  effort: AgentEffort = "auto"
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("run_agent_task", {
      input: { prompt, sessionId, currentTime: currentAgentTimeContext(), effort, attachments }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      sessionId,
      status: "failed",
      canCancel: false,
      canRetry: true,
      canContinue: false,
      timeline: [
        ...browserAgentState.timeline,
        {
          label: "Error",
          detail: "Browser preview cannot run the Rust agent loop. Open the Tauri app to execute tools.",
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      lastError: "Browser preview cannot run the Rust agent loop. Open the Tauri app to execute tools."
    };
    return browserAgentState;
  }
}

export async function queueAgentMessage(
  prompt: string,
  sessionId: string,
  attachments: AgentAttachment[] = [],
  effort: AgentEffort = "auto"
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("queue_agent_message", {
      input: { prompt, sessionId, currentTime: currentAgentTimeContext(), effort, attachments }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    const visiblePrompt =
      prompt.trim() || `Review attached ${attachments.map((attachment) => attachment.name).join(", ")}`;
    browserAgentState = {
      ...browserAgentState,
      sessionId,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: [
        ...browserAgentState.queuedMessages,
        {
          id: `agent-queue-${now}`,
          sessionId,
          prompt: visiblePrompt,
          attachments,
          effort,
          mode: "queue",
          createdAtMs: now,
          updatedAtMs: now
        }
      ]
    };
    return browserAgentState;
  }
}

export async function editQueuedAgentMessage(
  sessionId: string,
  queueId: string,
  prompt: string
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("edit_queued_agent_message", {
      input: { sessionId, queueId, prompt }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: browserAgentState.queuedMessages.map((message) =>
        message.id === queueId ? { ...message, prompt: prompt.trim(), updatedAtMs: now } : message
      )
    };
    return browserAgentState;
  }
}

export async function deleteQueuedAgentMessage(
  sessionId: string,
  queueId: string
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("delete_queued_agent_message", {
      input: { sessionId, queueId }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserAgentState = {
      ...browserAgentState,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: browserAgentState.queuedMessages.filter((message) => message.id !== queueId)
    };
    return browserAgentState;
  }
}

export async function steerQueuedAgentMessage(
  sessionId: string,
  queueId: string
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("steer_queued_agent_message", {
      input: { sessionId, queueId }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      status: browserAgentState.canCancel ? "cancelled" : browserAgentState.status,
      canCancel: false,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: browserAgentState.queuedMessages
        .map((message) =>
          message.id === queueId
            ? { ...message, mode: "steer" as const, updatedAtMs: now }
            : message
        )
        .sort((left, right) => {
          if (left.mode !== right.mode) return left.mode === "steer" ? -1 : 1;
          return left.createdAtMs - right.createdAtMs;
        })
    };
    return browserAgentState;
  }
}

export async function runNextQueuedAgentMessage(sessionId: string): Promise<AgentState | null> {
  try {
    return await invoke<AgentState | null>("run_next_queued_agent_message", {
      input: { sessionId }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const queued = browserAgentState.queuedMessages[0];
    if (!queued) return null;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      status: "failed",
      canCancel: false,
      canRetry: true,
      canContinue: false,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: browserAgentState.queuedMessages.slice(1),
      timeline: [
        ...browserAgentState.timeline,
        {
          label: "Error",
          detail: "Browser preview cannot run queued Rust agent work. Open the Tauri app to execute tools.",
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      lastError: "Browser preview cannot run queued Rust agent work. Open the Tauri app to execute tools."
    };
    return browserAgentState;
  }
}

export async function cancelAgentTask(sessionId: string): Promise<AgentState> {
  try {
    return await invoke<AgentState>("cancel_agent_task", { input: { sessionId } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      status: "cancelled",
      canCancel: false,
      canRetry: true,
      canContinue: false,
      pendingApprovals: [],
      timeline: [
        ...browserAgentState.timeline,
        {
          label: "Status",
          detail: "Agent task cancelled",
          kind: "message",
          state: "done",
          timestampMs: now
        }
      ],
      lastError: null
    };
    return browserAgentState;
  }
}

export async function retryAgentTask(sessionId: string): Promise<AgentState> {
  try {
    return await invoke<AgentState>("retry_agent_task", { input: { sessionId } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      status: "failed",
      canCancel: false,
      canRetry: true,
      canContinue: false,
      timeline: [
        ...browserAgentState.timeline,
        {
          label: "Error",
          detail: "Browser preview cannot retry the Rust agent loop. Open the Tauri app to execute tools.",
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      lastError: "Browser preview cannot retry the Rust agent loop. Open the Tauri app to execute tools."
    };
    return browserAgentState;
  }
}

export async function resolveAgentPermission(
  requestId: string,
  decision: "allow_once" | "allow_for_session" | "deny",
  sessionId: string
): Promise<AgentState> {
  try {
    return await invoke<AgentState>("resolve_agent_permission", {
      requestId,
      decision,
      sessionId
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      status: browserAgentState.pendingApprovals.length > 1 ? "waiting_for_permission" : "running",
      canCancel: true,
      canRetry: false,
      canContinue: false,
      pendingApprovals: browserAgentState.pendingApprovals.filter(
        (candidate) => candidate.requestId !== requestId
      ),
      timeline: [
        ...browserAgentState.timeline,
        {
          label: "Permission resolved",
          detail: `Agent permission ${decision}`,
          kind: "permission",
          state: "done",
          timestampMs: now
        }
      ],
      lastError: null
    };
    return browserAgentState;
  }
}

export async function getPhase5State(): Promise<Phase5State> {
  try {
    return await invoke<Phase5State>("get_phase5_state");
  } catch {
    return browserPhase5State;
  }
}

export async function runTool(toolName: string, input: string): Promise<Phase5State> {
  try {
    return await invoke<Phase5State>("run_tool", { input: { toolName, input } });
  } catch {
    const now = Date.now();
    if (toolName === "file.write" || toolName === "shell.run") {
      const requestId = `perm-browser-${now}`;
      browserPhase5State = {
        ...browserPhase5State,
        pendingApprovals: [
          {
            requestId,
            invocationId: `tool-browser-${now}`,
            toolName,
            risk: toolName === "shell.run" ? "execute" : "write",
            reason: toolName === "shell.run" ? "Run a local process." : "Write a workspace file.",
            scope: ".",
            input,
            requestedAtMs: now
          },
          ...browserPhase5State.pendingApprovals
        ],
        timeline: [
          ...browserPhase5State.timeline,
          {
            label: "Permission requested",
            detail: `Permission requested for ${toolName}`,
            kind: "permission",
            state: "pending",
            timestampMs: now
          }
        ],
        lastError: null
      };
      return browserPhase5State;
    }

    browserPhase5State = {
      ...browserPhase5State,
      results: [
        {
          invocationId: `tool-browser-${now}`,
          toolName,
          status: "succeeded",
          output: "Browser preview cannot execute Rust tools. Open the Tauri app to run this for real.",
          timestampMs: now
        },
        ...browserPhase5State.results
      ],
      lastError: null
    };
    return browserPhase5State;
  }
}

export async function resolveToolPermission(
  requestId: string,
  decision: "allow_once" | "allow_for_session" | "deny"
): Promise<Phase5State> {
  try {
    return await invoke<Phase5State>("resolve_tool_permission", { requestId, decision });
  } catch {
    const now = Date.now();
    const approval = browserPhase5State.pendingApprovals.find(
      (candidate) => candidate.requestId === requestId
    );
    browserPhase5State = {
      ...browserPhase5State,
      pendingApprovals: browserPhase5State.pendingApprovals.filter(
        (candidate) => candidate.requestId !== requestId
      ),
      results: approval
        ? [
            {
              invocationId: approval.invocationId,
              toolName: approval.toolName,
              status: decision === "deny" ? "denied" : "succeeded",
              output:
                decision === "deny"
                  ? "denied by user"
                  : "Browser preview approval recorded. Open the Tauri app to execute real tools.",
              timestampMs: now
            },
            ...browserPhase5State.results
          ]
        : browserPhase5State.results,
      timeline: [
        ...browserPhase5State.timeline,
        {
          label: "Permission resolved",
          detail: `Permission ${decision === "deny" ? "denied" : "approved"} with ${decision}`,
          kind: "permission",
          state: "done",
          timestampMs: now
        }
      ],
      lastError: null
    };
    return browserPhase5State;
  }
}

export async function getPhase6State(): Promise<Phase6State> {
  try {
    return await invoke<Phase6State>("get_phase6_state");
  } catch {
    return browserPhase6State;
  }
}

export async function runOrchestration(policy: string, prompt: string): Promise<Phase6State> {
  try {
    return await invoke<Phase6State>("run_orchestration", { input: { policy, prompt } });
  } catch {
    const now = Date.now();
    browserPhase6State = {
      ...browserPhase6State,
      timeline: [
        ...browserPhase6State.timeline,
        {
          label: "Error",
          detail: "Browser preview cannot run orchestration. Open the Tauri app to call the provider.",
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      lastError: "Browser preview cannot run orchestration. Open the Tauri app to call the provider."
    };
    return browserPhase6State;
  }
}

export async function getPhase7State(): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("get_phase7_state");
  } catch {
    return browserPhase7State;
  }
}

export async function indexWorkspaceRag(): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("index_workspace_rag");
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserPhase7State = {
      ...browserPhase7State,
      timeline: [
        ...browserPhase7State.timeline,
        {
          label: "Retrieval",
          detail: "Workspace indexed for RAG",
          kind: "tool",
          state: "done",
          timestampMs: now
        }
      ],
      stats: {
        filesIndexed: 1,
        chunksIndexed: 1,
        indexedAtMs: now
      },
      lastError: null
    };
    return browserPhase7State;
  }
}

export async function searchRag(query: string, limit = 6): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("search_rag", { input: { query, limit } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserPhase7State = {
      ...browserPhase7State,
      timeline: [
        ...browserPhase7State.timeline,
        {
          label: "Retrieval",
          detail: `RAG search completed for ${query}`,
          kind: "tool",
          state: "done",
          timestampMs: now
        }
      ],
      sources: [
        {
          path: "browser-preview.md",
          startLine: 1,
          endLine: 3,
          fileHash: "preview",
          score: 1,
          reason: "browser_preview",
          text: "Browser preview cannot inspect the local RAG index. Open the Tauri app to search real workspace chunks."
        }
      ],
      answer: null,
      lastError: null
    };
    return browserPhase7State;
  }
}

export async function answerWithRag(query: string, limit = 6): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("answer_with_rag", { input: { query, limit } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserPhase7State = {
      ...browserPhase7State,
      timeline: [
        ...browserPhase7State.timeline,
        {
          label: "Error",
          detail: "Browser preview cannot call the configured RAG answer model.",
          kind: "message",
          state: "pending",
          timestampMs: now
        }
      ],
      answer: null,
      lastError: "Browser preview cannot call the configured RAG answer model."
    };
    return browserPhase7State;
  }
}

export async function getPhase8State(): Promise<Phase8State> {
  try {
    return await invoke<Phase8State>("get_phase8_state");
  } catch {
    return browserPhase8State;
  }
}

export async function getContextState(sessionId?: string): Promise<ContextState> {
  try {
    return await invoke<ContextState>("get_context_state", {
      sessionId: sessionId ?? null
    });
  } catch {
    return browserContextState;
  }
}

export async function compactContext(): Promise<ContextState> {
  try {
    return await invoke<ContextState>("compact_context");
  } catch {
    const now = Date.now();
    browserContextState = {
      timeline: [
        ...browserContextState.timeline,
        {
          label: "Status",
          detail: "Context checkpoint compacted",
          kind: "message",
          state: "done",
          timestampMs: now
        }
      ],
      checkpoint: createBrowserContextCheckpoint(".cindx/context-checkpoint.md"),
      lastError: null
    };
    return browserContextState;
  }
}

export async function runBrowserTool(toolName: string, input: string): Promise<Phase8State> {
  try {
    return await invoke<Phase8State>("run_browser_tool", { input: { toolName, input } });
  } catch {
    const now = Date.now();
    const requestId = `perm-browser-${now}`;
    browserPhase8State = {
      ...browserPhase8State,
      pendingApprovals: [
        {
          requestId,
          invocationId: `browser-preview-${now}`,
          toolName,
          risk: "network",
          reason: "Use a network or browser observation tool.",
          scope: input,
          input,
          requestedAtMs: now
        },
        ...browserPhase8State.pendingApprovals
      ],
      timeline: [
        ...browserPhase8State.timeline,
        {
          label: "Permission requested",
          detail: `Permission requested for ${toolName}`,
          kind: "permission",
          state: "pending",
          timestampMs: now
        }
      ],
      lastError: null
    };
    return browserPhase8State;
  }
}

export async function resolveBrowserPermission(
  requestId: string,
  decision: "allow_once" | "allow_for_session" | "deny"
): Promise<Phase8State> {
  try {
    return await invoke<Phase8State>("resolve_browser_permission", { requestId, decision });
  } catch {
    const now = Date.now();
    const approval = browserPhase8State.pendingApprovals.find(
      (candidate) => candidate.requestId === requestId
    );
    browserPhase8State = {
      ...browserPhase8State,
      pendingApprovals: browserPhase8State.pendingApprovals.filter(
        (candidate) => candidate.requestId !== requestId
      ),
      observations: approval
        ? [
            {
              invocationId: approval.invocationId,
              toolName: approval.toolName,
              status: decision === "deny" ? "denied" : "succeeded",
              url: null,
              output:
                decision === "deny"
                  ? "denied by user"
                  : "Browser preview approval recorded. Open the Tauri app to run browser tools.",
              artifactPath: null,
              textPath: null,
              captureKind: null,
              timestampMs: now
            },
            ...browserPhase8State.observations
          ]
        : browserPhase8State.observations,
      timeline: [
        ...browserPhase8State.timeline,
        {
          label: "Permission resolved",
          detail: `Permission ${decision === "deny" ? "denied" : "approved"} with ${decision}`,
          kind: "permission",
          state: "done",
          timestampMs: now
        }
      ],
      lastError: null
    };
    return browserPhase8State;
  }
}

export async function subscribeToModelStream(
  onDelta: (payload: ModelStreamDelta) => void
): Promise<() => void> {
  try {
    return await listen<ModelStreamDelta>("model-stream-delta", (event) => {
      onDelta(event.payload);
    });
  } catch {
    return () => {};
  }
}

export async function readArtifactImage(path: string): Promise<string> {
  return invoke<string>("read_artifact_image", { path });
}

export async function readArtifactPreview(path: string): Promise<ArtifactPreview> {
  return invoke<ArtifactPreview>("read_artifact_preview", { path });
}

export async function openArtifact(path: string): Promise<void> {
  return invoke<void>("open_artifact", { path });
}

export async function revealArtifact(path: string): Promise<void> {
  return invoke<void>("reveal_artifact", { path });
}

export async function openExternalUrl(url: string): Promise<void> {
  return invoke<void>("open_external_url", { url });
}

function createBrowserContextCheckpoint(path: string | null): ContextCheckpointView {
  const now = Date.now();
  const restorePack = [
    "# Cindx Context Checkpoint",
    "",
    `- Checkpoint: browser-preview-${now}`,
    `- Generated: ${now}`,
    "- Events: 0",
    "- Tasks: 0",
    "",
    "## Current Goal",
    "- Browser preview cannot read the Rust event log. Open the Tauri app to generate a real restore pack.",
    "",
    "## Next Actions",
    "- Compact context inside the desktop app after running local agent actions.",
    ""
  ].join("\n");

  return {
    id: `browser-preview-${now}`,
    generatedAtMs: now,
    eventCount: 0,
    taskCount: 0,
    latestEventMs: now,
    currentGoal: "Open the Tauri app to generate a real restore pack.",
    completedSteps: [],
    pendingSteps: [],
    decisions: [],
    fileChanges: [],
    commandsRun: [],
    toolResults: [],
    retrievals: [],
    artifacts: [],
    errors: [],
    nextActions: ["Compact context inside the desktop app after running local agent actions."],
    path,
    restorePack
  };
}
