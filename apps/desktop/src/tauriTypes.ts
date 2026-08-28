import type { ProviderId } from "./providerProfiles";
import type { AgentEffort, AgentRunBudgets } from "./agentRunBudgetModel";
export type { ProviderId } from "./providerProfiles";

export type RuntimeStatus = {
  appVersion: string;
  kernelStatus: string;
  providerReady: boolean;
  workspaceRoot: string;
  orchestrationModes: string[];
  registeredTools: string[];
  agentRunBudgets: AgentRunBudgets | null;
};

export type PersonalizationConfig = {
  preferredName: string;
  responseTone: "natural" | "warm" | "professional" | "direct";
  responseLength: "concise" | "balanced" | "detailed";
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
export type WorkspaceUndoEntryView = {
  toolCallId: string; sequence: number; tool: string; path: string; action: string; undone: boolean; undoable: boolean;
};
export type WorkspaceUndoState = { sessionId: string; entries: WorkspaceUndoEntryView[]; canUndo: boolean; canRedo: boolean; };
export type CustomCommandView = { name: string; description: string; effort: string | null; template: string; scope: string; };
export type CustomCommandsState = { schema: string; commands: CustomCommandView[]; lastError: string | null; };

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
  /** The model the user picked in the composer; empty means the provider default applies. */
  agentModel: string;
  status: string;
  titleState: "pending" | "automatic" | "manual";
  activity: "idle" | "working" | "complete" | "attention";
  attentionReason: string | null;
  unseenResult: boolean;
  latestSequence: number;
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

export type ScheduleCadence = "once" | "daily" | "weekdays" | "weekly";

export type ScheduleRun = {
  id: string;
  queueId: string | null;
  source: "scheduled" | "manual";
  scheduledForMs: number;
  queuedAtMs: number | null;
  dispatchAttempts: number;
  lastDispatchAtMs: number | null;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  status:
    | "preparing"
    | "queued"
    | "running"
    | "waiting_for_permission"
    | "paused"
    | "completed"
    | "failed"
    | "cancelled"
    | "skipped";
  error: string | null;
};

export type ScheduleView = {
  id: string;
  name: string;
  projectId: string | null;
  projectName: string;
  sessionId: string | null;
  sessionName: string;
  prompt: string;
  effort: AgentEffort;
  timezone: string;
  cadence: ScheduleCadence;
  anchorAtMs: number;
  weeklyDays: number[];
  endsAtMs: number | null;
  catchUp: boolean;
  enabled: boolean;
  nextRunAtMs: number | null;
  createdAtMs: number;
  updatedAtMs: number;
  runs: ScheduleRun[];
};

export type ScheduleState = {
  schedules: ScheduleView[];
  lastError: string | null;
};

export type UpsertScheduleInput = {
  id?: string | null;
  name: string;
  projectId: string | null;
  sessionId: string | null;
  prompt: string;
  effort: AgentEffort;
  timezone: string;
  cadence: ScheduleCadence;
  anchorLocal: string;
  weeklyDays: number[];
  endsLocal: string | null;
  catchUp: boolean;
  enabled: boolean;
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
  planMode: boolean;
  createdAtMs: number;
  updatedAtMs: number;
};

export type QueuedAgentMessageReceipt = {
  message: QueuedAgentMessage;
  eventCount: number;
  latestSequence: number;
  latestTimestampMs: number;
};

export type QueuedAgentMessageActionReceipt = {
  queueId: string;
  message: QueuedAgentMessage | null;
  eventCount: number;
  latestSequence: number;
  latestTimestampMs: number;
  cancelledActiveRun: boolean;
  steerCommitted: boolean;
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
  toolName?: string;
  state: "done" | "pending" | "idle";
  timestampMs: number;
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
  providerId: ProviderId;
  providerResource: string;
  baseUrl: string;
  model: string;
  conductorModel: string;
  plannerModel: string;
  executorModel: string;
  reviewerModel: string;
  summarizerModel: string;
  fastModel: string;
  autoModel: string;
  proModel: string;
  embeddingModel: string;
  imageModel: string;
  imageEndpoint: string;
  voiceModel: string;
  collaborationPolicy: string;
  /** Fail-closed delivery judge: a judged-but-unresolved mutation-bearing run fails instead of delivering the unverified candidate. */
  directJudgeFailClosed: boolean;
  contextWindowTokens: number;
  agentSystemPrompt: string;
  ready: boolean;
  apiKeySet: boolean;
  authVerified: boolean;
  authVerifiedAtMs: number | null;
  /** The models the user enabled in Settings; the composer offers exactly these. Empty = all catalog models. */
  enabledModels: string[];
};

export type ProviderConfigInput = {
  providerId: ProviderId;
  providerResource: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  conductorModel: string;
  plannerModel: string;
  executorModel: string;
  reviewerModel: string;
  summarizerModel: string;
  fastModel: string;
  autoModel: string;
  proModel: string;
  embeddingModel: string;
  imageModel: string;
  imageEndpoint: string;
  voiceModel: string;
  collaborationPolicy: string;
  /** Fail-closed delivery judge toggle; omitted by older clients means fail-open. */
  directJudgeFailClosed: boolean;
  contextWindowTokens: number;
  agentSystemPrompt: string;
  /** The models the user enabled in Settings; the composer offers exactly these. Empty = all catalog models. */
  enabledModels: string[];
};

export type ProviderModelsState = {
  models: string[];
  fetchedAtMs: number;
  lastError: string | null;
};

export type VoiceSessionAnswer = {
  answerSdp: string;
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
  runId?: string | null;
  queueId?: string | null;
  attachments?: AgentAttachment[];
};

export type Phase4State = {
  provider: ProviderConfigState;
  timeline: TimelineEntry[];
  messages: ChatMessageView[];
  lastError: string | null;
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
  canAllowSession: boolean;
  /** True when a write subagent raised the request inside its live parent run. */
  subagent: boolean;
};

export type Phase5State = {
  timeline: TimelineEntry[];
  tools: ToolSpecView[];
  pendingApprovals: ToolApprovalView[];
  results: ToolRunView[];
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
  observedUses: number;
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

export type PendingPlanConfirmationView = {
  planMarkdown: string;
  planDigest: string;
  proposedAtMs: number;
};

export type AgentState = {
  taskId: string;
  projectId: string | null;
  projectName: string | null;
  sessionId: string | null;
  sessionName: string | null;
  status:
    | "idle"
    | "running"
    | "waiting_for_permission"
    | "paused"
    | "completed"
    | "failed"
    | "cancelled";
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
  pendingPlanConfirmation?: PendingPlanConfirmationView | null;
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
  interrupted: number;
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
  kind: "image" | "file" | "directory";
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
