import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import desktopPackage from "../package.json" with { type: "json" };
import { providerApiKeySetAfterSave, resolveProviderProfile } from "./providerProfiles.ts";
import { normalizeApprovalPolicy } from "./approvalPolicyModel.ts";
import * as projectMemory from "./memoryManagementModel.ts";
import {
  decodeNativeAgentState,
  decodeNativeAgentStateDelta
} from "./tauriAgentStateContract.ts";
import { decodeNativeRuntimeStatus } from "./tauriRuntimeContract.ts";

export const DESKTOP_VERSION = desktopPackage.version;

export type * from "./tauriTypes";
export type * from "./ragOperationModel";
export type * from "./agentRunBudgetModel";
export type * from "./memoryManagementModel";
export type * from "./planModeModel";
export { stageAgentAttachments } from "./attachmentIpc.ts";
import type { AgentEffort } from "./agentRunBudgetModel";
import type { PlanConfirmationDecision, PlanResolvedBy } from "./planModeModel";
import type { NativeAgentHistoryPage, NativeAgentState, NativeAgentStateDelta } from "./tauriNativeTypes";
import type {
  RuntimeStatus,
  PersonalizationConfig,
  SessionSandboxMode,
  SidecarEndpointState,
  SidecarState,
  SidecarConfigInput,
  WebSearchConfigState,
  WebSearchConfigInput,
  McpTransportConfig,
  McpServerConfig,
  McpServerView,
  McpState,
  SkillRecord,
  SkillState,
  WorkspaceUndoState,
  CustomCommandsState,
  ProjectView,
  SessionView,
  ProjectSessionState,
  ScheduleCadence,
  ScheduleRun,
  ScheduleView,
  ScheduleState,
  UpsertScheduleInput,
  AgentAttachment,
  QueuedAgentMessage,
  QueuedAgentMessageReceipt,
  QueuedAgentMessageActionReceipt,
  ArtifactPreview,
  TimelineEntry,
  PermissionAudit,
  Phase3State,
  PermissionReviewItem,
  PermissionReviewState,
  ProviderConfigState,
  ProviderConfigInput,
  ProviderModelsState,
  ImageEndpointValidationState,
  ChatMessageView,
  Phase4State,
  ToolSpecView,
  ToolRunView,
  ToolApprovalView,
  Phase5State,
  RagStatsView,
  MemoryStatsView,
  RagSourceView,
  RetrievalChannelView,
  RetrievalTraceView,
  GraphNodeView,
  GraphEdgeView,
  GraphStateView,
  Phase7State,
  BrowserObservationView,
  Phase8State,
  ContextCheckpointView,
  ContextState,
  AgentState,
  AgentStateRevision,
  AgentStateDelta,
  AgentHistoryPage,
  AgentTraceStepView,
  AgentTraceTurnView,
  AgentTraceRoleSummary,
  AgentTraceState,
  AgentOutputArtifactView,
  ModelStreamDelta, VoiceSessionAnswer
} from "./tauriTypes";
import type { RagOperationProgress } from "./ragOperationModel";

import { createBrowserContextCheckpoint, createInitialAgentState, createInitialAgentTraceState, createInitialContextState, createInitialPersonalizationConfig, createInitialPhase3State, createInitialPhase4State, createInitialPhase5State, createInitialPhase7State, createInitialPhase8State, createInitialProjectSessionState, createInitialScheduleState, createInitialSidecarState, createInitialWebSearchConfig, newBrowserSessionId } from "./browserPreviewFallbackState.ts";

const browserDefaultSessionId = newBrowserSessionId();
let browserPhase3State: Phase3State = createInitialPhase3State();
let browserSidecarState: SidecarState = createInitialSidecarState();
let browserWebSearchConfig: WebSearchConfigState = createInitialWebSearchConfig();
let browserPersonalizationConfig: PersonalizationConfig = createInitialPersonalizationConfig();
let browserProjectSessionState: ProjectSessionState = createInitialProjectSessionState(browserDefaultSessionId);
let browserScheduleState: ScheduleState = createInitialScheduleState();
let browserPhase4State: Phase4State = createInitialPhase4State();
let browserPhase5State: Phase5State = createInitialPhase5State();
let browserPhase7State: Phase7State = createInitialPhase7State();
let browserPhase8State: Phase8State = createInitialPhase8State();
let browserContextState: ContextState = createInitialContextState();
let browserAgentState: AgentState = createInitialAgentState(browserDefaultSessionId);
let browserAgentTraceState: AgentTraceState = createInitialAgentTraceState(browserDefaultSessionId);

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function requireBrowserPreviewFallback(error: unknown) {
  if (isTauriRuntime()) throw error;
}

export async function revealMainWindow(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("reveal_main_window");
}

/** Best-effort crash telemetry for the top-level error boundary; never throws. */
export async function reportFrontendCrash(message: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await invoke<void>("report_frontend_crash", { message });
  } catch {
    // Telemetry must never mask the recovery UI.
  }
}

export async function setSidebarMaterialWidth(width: number): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("set_sidebar_material_width", { width });
}

function browserRuntimeStatus(workspaceRoot: string): RuntimeStatus {
  return {
    appVersion: DESKTOP_VERSION,
    kernelStatus: "browser preview",
    providerReady: browserPhase4State.provider.ready,
    workspaceRoot,
    orchestrationModes: ["single", "plan_execute_review", "best_of_n", "auto_router"],
    agentRunBudgets: null,
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

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  try {
    return decodeNativeRuntimeStatus(await invoke<unknown>("get_runtime_status"));
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserRuntimeStatus(".");
  }
}

export async function getPersonalizationConfig(): Promise<PersonalizationConfig> {
  try {
    return await invoke<PersonalizationConfig>("get_personalization_config");
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserPersonalizationConfig;
  }
}

export async function savePersonalizationConfig(
  input: PersonalizationConfig
): Promise<PersonalizationConfig> {
  try {
    return await invoke<PersonalizationConfig>("save_personalization_config", { input });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserPersonalizationConfig = { ...input };
    return browserPersonalizationConfig;
  }
}

export async function saveWorkspaceRoot(path: string): Promise<RuntimeStatus> {
  try {
    return decodeNativeRuntimeStatus(
      await invoke<unknown>("save_workspace_root", { input: { path } })
    );
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserRuntimeStatus(path);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
    requireBrowserPreviewFallback(error);
    return { servers: [], lastError: String(error) };
  }
}

export async function saveMcpServers(servers: McpServerConfig[]): Promise<McpState> {
  return await invoke<McpState>("save_mcp_servers", { input: { servers } });
}

export async function upsertMcpServer(server: McpServerConfig): Promise<McpState> {
  return await invoke<McpState>("upsert_mcp_server", { input: { server } });
}

export async function importExternalMcpServers(): Promise<McpState> {
  try {
    return await invoke<McpState>("import_external_mcp_servers");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return { servers: [], lastError: String(error) };
  }
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
    requireBrowserPreviewFallback(error);
    return { skills: [], lastError: String(error) };
  }
}

export async function refreshSkills(): Promise<SkillState> {
  return await invoke<SkillState>("refresh_skills");
}

export async function getWorkspaceUndoState(
  sessionId: string
): Promise<WorkspaceUndoState> {
  try {
    return await invoke<WorkspaceUndoState>("get_workspace_undo_state", { sessionId });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return { sessionId, entries: [], canUndo: false, canRedo: false };
  }
}

export async function undoWorkspaceChange(
  sessionId: string
): Promise<WorkspaceUndoState> {
  return await invoke<WorkspaceUndoState>("undo_workspace_change", { sessionId });
}

export async function redoWorkspaceChange(
  sessionId: string
): Promise<WorkspaceUndoState> {
  return await invoke<WorkspaceUndoState>("redo_workspace_change", { sessionId });
}

export async function getCustomCommands(): Promise<CustomCommandsState> {
  try {
    return await invoke<CustomCommandsState>("get_custom_commands");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return { schema: "cindx.custom-commands.v1", commands: [], lastError: String(error) };
  }
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserProjectSessionState;
  }
}

export async function acknowledgeSessionActivity(sessionId: string, throughSequence?: number) {
  try {
    return await invoke<ProjectSessionState>("acknowledge_session_activity", {
      input: { sessionId, throughSequence }
    });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId &&
        (throughSequence === undefined || session.latestSequence <= throughSequence)
          ? {
              ...session,
              activity: session.activity === "complete" ? "idle" : session.activity,
              unseenResult: false,
              status: session.activity === "complete" ? "Ready" : session.status
            }
          : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function confirmDeleteAction(
  kind: "project" | "session" | "sessions",
  name: string
): Promise<boolean> {
  try {
    return await invoke<boolean>("confirm_delete_action", { input: { kind, name } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return window.confirm(`Delete ${kind} “${name}”?`);
  }
}

export async function getScheduleState(): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("get_schedule_state");
  } catch (error) {
    if (isTauriRuntime()) throw error;
    return browserScheduleState;
  }
}

function browserScheduleTarget(input: UpsertScheduleInput) {
  const project = browserProjectSessionState.projects.find(
    (candidate) => candidate.id === input.projectId
  );
  const session = browserProjectSessionState.sessions.find(
    (candidate) => candidate.id === input.sessionId
  );
  return {
    projectName: project?.name ?? "No project",
    sessionName: session?.name ?? "Standalone"
  };
}

export async function upsertSchedule(input: UpsertScheduleInput): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("upsert_schedule", { input });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    const anchorAtMs = new Date(input.anchorLocal).getTime();
    const current = input.id
      ? browserScheduleState.schedules.find((schedule) => schedule.id === input.id)
      : null;
    const target = browserScheduleTarget(input);
    const next: ScheduleView = {
      id: current?.id ?? `schedule-${now}`,
      name: input.name.trim(),
      projectId: input.projectId,
      projectName: target.projectName,
      sessionId: input.sessionId,
      sessionName: target.sessionName,
      prompt: input.prompt.trim(),
      effort: input.effort,
      timezone: input.timezone,
      cadence: input.cadence,
      anchorAtMs,
      weeklyDays: input.weeklyDays,
      endsAtMs: input.endsLocal ? new Date(input.endsLocal).getTime() : null,
      catchUp: input.catchUp,
      enabled: input.enabled,
      nextRunAtMs: input.enabled ? anchorAtMs : null,
      createdAtMs: current?.createdAtMs ?? now,
      updatedAtMs: now,
      runs: current?.runs ?? []
    };
    browserScheduleState = {
      schedules: [
        ...browserScheduleState.schedules.filter((schedule) => schedule.id !== next.id),
        next
      ],
      lastError: null
    };
    return browserScheduleState;
  }
}

export async function setScheduleEnabled(
  scheduleId: string,
  enabled: boolean
): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("set_schedule_enabled", {
      input: { scheduleId, enabled }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserScheduleState = {
      ...browserScheduleState,
      schedules: browserScheduleState.schedules.map((schedule) =>
        schedule.id === scheduleId
          ? {
              ...schedule,
              enabled,
              nextRunAtMs: enabled ? schedule.anchorAtMs : null,
              updatedAtMs: Date.now()
            }
          : schedule
      )
    };
    return browserScheduleState;
  }
}

export async function deleteSchedule(scheduleId: string): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("delete_schedule", { input: { scheduleId } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserScheduleState = {
      ...browserScheduleState,
      schedules: browserScheduleState.schedules.filter(
        (schedule) => schedule.id !== scheduleId
      )
    };
    return browserScheduleState;
  }
}

export async function runScheduleNow(scheduleId: string): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("run_schedule_now", { input: { scheduleId } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserScheduleState = {
      ...browserScheduleState,
      schedules: browserScheduleState.schedules.map((schedule) =>
        schedule.id === scheduleId
          ? {
              ...schedule,
              runs: [
                ...schedule.runs,
                {
                  id: `schedule-run-${now}`,
                  queueId: null,
                  source: "manual" as const,
                  scheduledForMs: now,
                  queuedAtMs: now,
                  dispatchAttempts: 1,
                  lastDispatchAtMs: now,
                  startedAtMs: now,
                  finishedAtMs: now,
                  status: "completed" as const,
                  error: null
                }
              ]
            }
          : schedule
      )
    };
    return browserScheduleState;
  }
}

export async function cancelScheduleRun(scheduleId: string): Promise<ScheduleState> {
  try {
    return await invoke<ScheduleState>("cancel_schedule_run", { input: { scheduleId } });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserScheduleState = {
      ...browserScheduleState,
      schedules: browserScheduleState.schedules.map((schedule) =>
        schedule.id === scheduleId
          ? {
              ...schedule,
              runs: schedule.runs.map((run, index) =>
                index === schedule.runs.length - 1 &&
                ["preparing", "queued", "running", "waiting_for_permission"].includes(
                  run.status
                )
                  ? { ...run, status: "cancelled" as const, finishedAtMs: now }
                  : run
              )
            }
          : schedule
      )
    };
    return browserScheduleState;
  }
}

export async function createProject(name: string, root: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("create_project", { input: { name, root } });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    const now = Date.now();
    const suffix = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "project";
    const projectId = `project-${suffix}-${now}`;
    const sessionId = newBrowserSessionId();
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
            effort: "default",
            agentModel: "",
            status: "Ready",
            titleState: "pending",
            activity: "idle",
            attentionReason: null,
            unseenResult: false,
            latestSequence: 0,
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    const now = Date.now();
    const activeProjectId = projectId ?? browserProjectSessionState.activeProjectId;
    const sessionId = newBrowserSessionId();
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
            effort: "default",
            agentModel: "",
            status: "Ready",
            titleState: "pending",
            activity: "idle",
            attentionReason: null,
            unseenResult: false,
            latestSequence: 0,
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    const source = browserProjectSessionState.sessions.find((session) => session.id === sessionId);
    if (!source) return browserProjectSessionState;
    const now = Date.now();
    const fork = {
      ...source,
      id: newBrowserSessionId(),
      name: `${source.name} Fork`,
      detail: `Fork of ${source.name}`,
      effort: "default" as AgentEffort,
      status: "Ready",
      titleState: "manual" as const,
      activity: "idle" as const,
      attentionReason: null,
      unseenResult: false,
      latestSequence: 0,
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
          ? { ...session, name: normalizedName, titleState: "manual", updatedAtMs: now }
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId ? { ...session, effort } : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function setSessionModel(
  sessionId: string,
  agentModel: string
): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("set_session_model", {
      input: { sessionId, agentModel }
    });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId ? { ...session, agentModel } : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function getSessionSandboxMode(sessionId: string): Promise<SessionSandboxMode> {
  return await invoke<SessionSandboxMode>("get_session_sandbox_mode", { sessionId });
}

export async function setSessionSandboxMode(
  sessionId: string,
  mode: SessionSandboxMode
): Promise<SessionSandboxMode> {
  return await invoke<SessionSandboxMode>("set_session_sandbox_mode", { sessionId, mode });
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return await getProjectSessionState();
  }
}

export async function removeAgentAttachment(sessionId: string, path: string): Promise<void> {
  await invoke<void>("remove_agent_attachment", { input: { sessionId, path } });
}

export async function archiveSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("archive_session", { input: { sessionId } });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    const source = browserProjectSessionState.sessions.find((session) => session.id === sessionId);
    if (!source) return browserProjectSessionState;
    const now = Date.now();
    let next: ProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId
          ? {
              ...session,
              active: false,
              archived: true,
              archivedAtMs: now,
              status: "Archived",
              activity: "idle",
              attentionReason: null,
              unseenResult: false
            }
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    browserProjectSessionState = {
      ...browserProjectSessionState,
      sessions: browserProjectSessionState.sessions.map((session) =>
        session.id === sessionId
          ? {
              ...session,
              archived: false,
              archivedAtMs: null,
              status: "Ready",
              activity: "idle",
              attentionReason: null,
              unseenResult: false
            }
          : session
      )
    };
    return browserProjectSessionState;
  }
}

export async function deleteSession(sessionId: string): Promise<ProjectSessionState> {
  try {
    return await invoke<ProjectSessionState>("delete_session", { input: { sessionId } });
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
    id: newBrowserSessionId(),
    projectId,
    name: "New Session",
    detail: "timeline + chat",
    effort: "default",
    agentModel: "",
    status: "Ready",
    titleState: "pending",
    activity: "idle",
    attentionReason: null,
    unseenResult: false,
    latestSequence: 0,
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserPhase3State;
  }
}

export async function getPermissionReviewState(): Promise<PermissionReviewState> {
  try {
    return await invoke<PermissionReviewState>("get_permission_review_state");
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
      canAllowSession: approval.canAllowSession
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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

export async function modelSupportsThinking(model: string): Promise<boolean> {
  try {
    return await invoke<boolean>("model_supports_thinking", { model });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return false;
  }
}

export async function getPhase4State(): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("get_phase4_state");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserPhase4State;
  }
}

export const negotiateVoiceSession = (offerSdp: string): Promise<VoiceSessionAnswer> => invoke<VoiceSessionAnswer>("negotiate_voice_session", { input: { offerSdp } });
export const transcribeVoiceAudio = (pcmBase64: string) => invoke<{ transcript: string }>("transcribe_voice_audio", { input: { pcmBase64 } });

export async function saveProviderConfig(input: ProviderConfigInput): Promise<Phase4State> {
  try {
    return await invoke<Phase4State>("save_provider_config", { input });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    const previousProvider = browserPhase4State.provider;
    const profile = resolveProviderProfile(input);
    const executorModel = input.executorModel || input.model;
    const apiKeySet = providerApiKeySetAfterSave({ ...input, ...profile }, previousProvider);
    browserPhase4State = {
      ...browserPhase4State,
      provider: {
        providerId: profile.providerId,
        providerResource: profile.providerResource,
        baseUrl: profile.baseUrl,
        model: input.model,
        conductorModel: input.conductorModel || input.plannerModel || input.model,
        plannerModel: input.plannerModel || input.model,
        executorModel,
        reviewerModel: input.reviewerModel || input.model,
        summarizerModel: input.summarizerModel || input.model,
        fastModel: input.fastModel,
        autoModel: input.autoModel,
        proModel: input.proModel,
        embeddingModel: input.embeddingModel || "text-embedding-3-small",
        imageModel: input.imageModel,
        imageEndpoint: profile.imageEndpoint,
        voiceModel: input.voiceModel,
        collaborationPolicy: input.collaborationPolicy || "auto_router",
        directJudgeFailClosed: input.directJudgeFailClosed === true,
        guardianAutoApproval: input.guardianAutoApproval === true,
        planFirstEnabled: input.planFirstEnabled === true,
        approvalPolicy: normalizeApprovalPolicy(input.approvalPolicy),
        contextWindowTokens: Math.max(4096, input.contextWindowTokens || 128000),
        agentSystemPrompt: input.agentSystemPrompt,
        ready: Boolean(profile.baseUrl.trim() && apiKeySet && executorModel.trim()),
        apiKeySet,
        authVerified: false, authVerifiedAtMs: null,
        enabledModels: input.enabledModels
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

export async function listProviderModels(input: Pick<ProviderConfigInput, "providerId" | "providerResource" | "baseUrl" | "apiKey">): Promise<ProviderModelsState> {
  try {
    return await invoke<ProviderModelsState>("list_provider_models", { input });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return {
      models: [],
      fetchedAtMs: 0,
      lastError: error instanceof Error ? error.message : String(error)
    };
  }
}

export async function validateImageEndpoint(input: Pick<ProviderConfigInput, "providerId" | "providerResource" | "baseUrl" | "imageModel" | "imageEndpoint">): Promise<ImageEndpointValidationState> {
  try {
    return await invoke<ImageEndpointValidationState>("validate_image_endpoint", { input });
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
    return decodeNativeAgentState(await invoke<unknown>("get_agent_state", {
      sessionId: sessionId ?? null
    }));
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
    return decodeNativeAgentStateDelta(await invoke<unknown>("get_agent_state_delta", {
      sessionId,
      afterSequence
    }));
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
    return await invoke<NativeAgentHistoryPage>("get_agent_history_page", {
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
  effort: AgentEffort = "default",
  planMode = false
): Promise<AgentState> {
  try {
    return decodeNativeAgentState(await invoke<unknown>("run_agent_task", {
      input: { prompt, sessionId, currentTime: currentAgentTimeContext(), effort, attachments, planMode }
    }));
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
  effort: AgentEffort = "default",
  queueId?: string,
  planMode = false
): Promise<QueuedAgentMessageReceipt> {
  try {
    return await invoke<QueuedAgentMessageReceipt>("queue_agent_message", {
      input: {
        prompt,
        sessionId,
        currentTime: currentAgentTimeContext(),
        effort,
        attachments,
        queueId,
        planMode
      }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    const visiblePrompt =
      prompt.trim() || `Review attached ${attachments.map((attachment) => attachment.name).join(", ")}`;
    const message: QueuedAgentMessage = {
      id: queueId || `agent-queue-${now}`,
      sessionId,
      prompt: visiblePrompt,
      attachments,
      effort,
      mode: "queue",
      planMode,
      createdAtMs: now,
      updatedAtMs: now
    };
    browserAgentState = {
      ...browserAgentState,
      sessionId,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: [...browserAgentState.queuedMessages, message]
    };
    return {
      message,
      eventCount: browserAgentState.eventCount,
      latestSequence: browserAgentState.latestSequence,
      latestTimestampMs: now
    };
  }
}

export async function editQueuedAgentMessage(
  sessionId: string,
  queueId: string,
  prompt: string
): Promise<QueuedAgentMessageActionReceipt> {
  try {
    return await invoke<QueuedAgentMessageActionReceipt>("edit_queued_agent_message", {
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
    return {
      queueId,
      message: browserAgentState.queuedMessages.find((message) => message.id === queueId) ?? null,
      eventCount: browserAgentState.eventCount,
      latestSequence: browserAgentState.latestSequence,
      latestTimestampMs: now,
      cancelledActiveRun: false, steerCommitted: false
    };
  }
}

export async function deleteQueuedAgentMessage(
  sessionId: string,
  queueId: string
): Promise<QueuedAgentMessageActionReceipt> {
  try {
    return await invoke<QueuedAgentMessageActionReceipt>("delete_queued_agent_message", {
      input: { sessionId, queueId }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1,
      queuedMessages: browserAgentState.queuedMessages.filter((message) => message.id !== queueId)
    };
    return {
      queueId,
      message: null,
      eventCount: browserAgentState.eventCount,
      latestSequence: browserAgentState.latestSequence,
      latestTimestampMs: now,
      cancelledActiveRun: false, steerCommitted: false
    };
  }
}

export async function steerQueuedAgentMessage(
  sessionId: string,
  queueId: string
): Promise<QueuedAgentMessageActionReceipt> {
  try {
    return await invoke<QueuedAgentMessageActionReceipt>("steer_queued_agent_message", {
      input: { sessionId, queueId }
    });
  } catch (error) {
    if (isTauriRuntime()) throw error;
    const now = Date.now();
    browserAgentState = {
      ...browserAgentState,
      eventCount: browserAgentState.eventCount + 1,
      latestSequence: browserAgentState.latestSequence + 1
    };
    return {
      queueId,
      message: browserAgentState.queuedMessages.find((message) => message.id === queueId) ?? null,
      eventCount: browserAgentState.eventCount,
      latestSequence: browserAgentState.latestSequence,
      latestTimestampMs: now,
      cancelledActiveRun: false,
      steerCommitted: false
    };
  }
}

export async function runNextQueuedAgentMessage(sessionId: string): Promise<AgentState | null> {
  try {
    const raw = await invoke<unknown>("run_next_queued_agent_message", {
      input: { sessionId }
    });
    return raw === null ? null : decodeNativeAgentState(raw);
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
    return decodeNativeAgentState(await invoke<unknown>("cancel_agent_task", { input: { sessionId } }));
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
    return decodeNativeAgentState(await invoke<unknown>("retry_agent_task", { input: { sessionId } }));
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
  sessionId: string,
  grantCommandPrefix = false
): Promise<AgentState> {
  try {
    return decodeNativeAgentState(await invoke<unknown>("resolve_agent_permission", {
      requestId,
      decision,
      sessionId,
      grantCommandPrefix
    }));
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

export async function resolveAgentPlanConfirmation(
  sessionId: string,
  decision: PlanConfirmationDecision,
  resolvedBy: PlanResolvedBy
): Promise<AgentState> {
  try {
    return decodeNativeAgentState(await invoke<unknown>("resolve_agent_plan_confirmation", {
      sessionId,
      decision,
      resolvedBy
    }));
  } catch (error) {
    if (isTauriRuntime()) throw error;
    browserAgentState = {
      ...browserAgentState,
      status: decision === "cancel" ? "cancelled" : "running",
      canCancel: decision !== "cancel",
      canContinue: false,
      pendingPlanConfirmation: null,
      lastError: null
    };
    return browserAgentState;
  }
}

export async function getPhase5State(): Promise<Phase5State> {
  try {
    return await invoke<Phase5State>("get_phase5_state");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserPhase5State;
  }
}

export async function runTool(toolName: string, input: string): Promise<Phase5State> {
  try {
    return await invoke<Phase5State>("run_tool", { input: { toolName, input } });
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
            requestedAtMs: now,
            canAllowSession: false,
            subagent: false
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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

export async function getPhase7State(): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("get_phase7_state");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserPhase7State;
  }
}

export const getProjectMemoryState = (projectId: string) =>
  invoke<projectMemory.ProjectMemoryState>("get_project_memory_state", { projectId }).catch((error) => { requireBrowserPreviewFallback(error); return projectMemory.getBrowserProjectMemoryState(projectId); });

export const updateProjectMemory = (input: projectMemory.UpdateProjectMemoryInput) =>
  invoke<projectMemory.ProjectMemoryState>("update_project_memory", { input }).catch((error) => { requireBrowserPreviewFallback(error); return projectMemory.updateBrowserProjectMemory(input); });
export const ensureWorkspaceKnowledge = (): Promise<Phase7State> =>
  invoke<Phase7State>("ensure_workspace_knowledge").catch((error) => {
    requireBrowserPreviewFallback(error); return browserPhase7State;
  });
export async function indexWorkspaceRag(operationId: string): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("index_workspace_rag", { input: { operationId } });
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

export async function searchRag(operationId: string, query: string, limit = 6): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("search_rag", { input: { operationId, query, limit } });
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

export async function answerWithRag(operationId: string, query: string, limit = 6): Promise<Phase7State> {
  try {
    return await invoke<Phase7State>("answer_with_rag", {
      input: { operationId, query, limit }
    });
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

export async function cancelRagOperation(operationId: string): Promise<boolean> {
  try {
    return await invoke<boolean>("cancel_rag_operation", { operationId });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return false;
  }
}

export async function getPhase8State(): Promise<Phase8State> {
  try {
    return await invoke<Phase8State>("get_phase8_state");
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserPhase8State;
  }
}

export async function getContextState(sessionId?: string): Promise<ContextState> {
  try {
    return await invoke<ContextState>("get_context_state", {
      sessionId: sessionId ?? null
    });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return browserContextState;
  }
}

export async function compactContext(): Promise<ContextState> {
  try {
    return await invoke<ContextState>("compact_context");
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
          requestedAtMs: now,
          canAllowSession: false,
          subagent: false
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
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
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return () => {};
  }
}

export async function subscribeToRagOperationProgress(onProgress: (payload: RagOperationProgress) => void): Promise<() => void> {
  try {
    return await listen<RagOperationProgress>("rag-operation-progress", (event) => {
      onProgress(event.payload);
    });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return () => {};
  }
}

export async function subscribeToSessionTitleUpdates(
  onUpdate: (sessionId: string) => void
): Promise<() => void> {
  try {
    return await listen<string>("session-title-updated", (event) => {
      onUpdate(event.payload);
    });
  } catch (error) {
    requireBrowserPreviewFallback(error);
    return () => {};
  }
}

export async function readArtifactImage(path: string): Promise<Uint8Array> {
  const bytes = await invoke<ArrayBuffer>("read_artifact_image", { path });
  return new Uint8Array(bytes);
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

