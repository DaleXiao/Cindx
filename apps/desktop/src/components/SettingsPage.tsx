import {
  ArchiveRestore,
  ArrowLeft,
  BookOpen,
  Bot,
  Bug,
  Cable,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Database,
  FolderOpen,
  KeyRound,
  LayoutDashboard,
  Link2,
  Monitor,
  Moon,
  PackagePlus,
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
  Suspense,
  type Dispatch,
  type RefObject,
  type SetStateAction
} from "react";
import {
  DESKTOP_VERSION,
  type ContextCheckpointView,
  type McpState,
  type PermissionReviewItem,
  type PersonalizationConfig,
  type Phase4State,
  type Phase5State,
  type Phase7State,
  type ProjectSessionState,
  type ProviderConfigInput,
  type RagOperationKind,
  type RagOperationProgress,
  type RagSourceView,
  type RagStatsView,
  type RuntimeStatus,
  type SessionView,
  type SidecarState,
  type SkillState,
  type ToolSpecView,
  type WebSearchConfigState
} from "../tauri";
import { DEBUG_ALWAYS_VISIBLE_STORAGE_KEY } from "../appShellModel";
import { useMemorySettingsController } from "../controllers/useMemorySettingsController";
import type { ProviderModelGroups } from "../providerProfiles";
import { SettingsMemoryPanel } from "./SettingsMemoryPanel";
import { SettingsModelsPanel } from "./SettingsModelsPanel";
import { SettingsPermissionsPanel } from "./SettingsPermissionsPanel";
import { SettingsToolsPanel, type WebSearchDraft } from "./SettingsToolsPanel";

const appIconUrl = new URL("../../src-tauri/icons/icon.png", import.meta.url).href;

const KnowledgeGraph = lazy(() =>
  import("./KnowledgeGraph").then((module) => ({
    default: module.KnowledgeGraph
  }))
);

export type AppearanceMode = "light" | "dark" | "system";

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

export type SettingsCategory = (typeof settingsCategories)[number]["id"];

type McpDraft = {
  name: string;
  command: string;
  args: string;
  envKey: string;
  envValue: string;
};

type SidecarDraft = {
  browserPath: string;
  computerPath: string;
  autoConfigure: boolean;
};

export type SettingsPageProps = {
  activeRagOperation: { id: string; kind: RagOperationKind } | null;
  canUseConfiguredKey: boolean;
  activePermissionReviews: PermissionReviewItem[];
  appearanceMode: AppearanceMode;
  archivedSessions: SessionView[];
  browserBusy: boolean;
  browserTarget: string;
  browserText: string;
  browserUrl: string;
  busySessionIds: Set<string>;
  modelProfileCount: number;
  contextBusy: boolean;
  contextCheckpoint: ContextCheckpointView | null;
  debugAlwaysVisible: boolean;
  flushPersonalization: (notify: boolean) => Promise<void>;
  handleAddMcpServer: () => Promise<void>;
  handleAnswerWithRag: () => Promise<void>;
  handleCancelRag: () => Promise<void>;
  handleAppearanceModeChange: (mode: AppearanceMode) => void;
  handleCompactContext: () => Promise<void>;
  handleIgnorePermissionReview: (requestId: string) => void;
  handleIndexRag: () => Promise<void>;
  handleInstallSkillPackage: (file: File) => Promise<void>;
  handleInstallSkillUrl: () => Promise<void>;
  handleLoadProviderModels: () => Promise<void>;
  handleReloadProviderState: () => Promise<unknown>;
  handleMcpPolicy: (serverId: string, enabled: boolean, requireApproval: boolean) => Promise<void>;
  handlePickWorkspace: () => Promise<void>;
  handlePromptEvolutionToggle: (enabled: boolean) => Promise<void>;
  handleRefreshMcpServer: (serverId: string) => Promise<void>;
  handleRefreshSkills: () => Promise<void>;
  handleRemoveMcpServer: (serverId: string) => Promise<void>;
  handleResolvePermissionReview: (
    review: PermissionReviewItem,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) => Promise<void>;
  handleRestorePermissionReview: (requestId: string) => void;
  handleRestoreSession: (sessionId: string) => Promise<void>;
  handleRunBrowserTool: (toolName: string) => Promise<void>;
  handleRunTool: () => Promise<void>;
  handleSavePersonalization: () => Promise<void>;
  handleSaveProviderConfig: () => Promise<void>;
  handleSaveSidecars: () => Promise<void>;
  handleSaveWebSearch: () => Promise<void>;
  handleSaveWorkspace: () => Promise<void>;
  handleSearchRag: () => Promise<void>;
  handleSkillPreference: (skillId: string, enabled: boolean, trusted: boolean) => Promise<void>;
  ignoredPermissionReviews: PermissionReviewItem[];
  imageEndpointValidation: "idle" | "checking" | "valid" | "invalid";
  knowledgeError: string | null;
  knowledgeGraphOpen: boolean;
  mcpBusy: boolean;
  mcpDraft: McpDraft;
  mcpState: McpState | null;
  permissionBusy: boolean;
  personalizationBusy: boolean;
  personalizationDraft: PersonalizationConfig;
  personalizationError: string | null;
  phase4: Phase4State | null;
  phase5: Phase5State | null;
  phase7: Phase7State | null;
  projectSessionBusy: boolean;
  projectSessionState: ProjectSessionState | null;
  providerBusy: boolean;
  providerDraft: ProviderConfigInput | null;
  providerModelOptions: ProviderModelGroups;
  providerModels: string[];
  providerModelsBusy: boolean;
  providerModelsError: string | null;
  providerModelsRefreshTurn: number;
  providerSettingsError: string | null;
  ragBusy: boolean;
  ragCancelling: boolean;
  ragProgress: RagOperationProgress | null;
  ragQuery: string;
  ragSources: RagSourceView[];
  ragStats: RagStatsView;
  runtime: RuntimeStatus | null;
  selectedTool: string;
  selectedToolSpec: ToolSpecView | undefined;
  setBrowserTarget: Dispatch<SetStateAction<string>>;
  setBrowserText: Dispatch<SetStateAction<string>>;
  setBrowserUrl: Dispatch<SetStateAction<string>>;
  setDebugAlwaysVisible: Dispatch<SetStateAction<boolean>>;
  setKnowledgeGraphOpen: (open: boolean) => void;
  setMcpDraft: Dispatch<SetStateAction<McpDraft>>;
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
  setRagQuery: Dispatch<SetStateAction<string>>;
  setSelectedTool: Dispatch<SetStateAction<string>>;
  setSettingsCategory: Dispatch<SetStateAction<SettingsCategory>>;
  setSidecarDraft: Dispatch<SetStateAction<SidecarDraft>>;
  setSkillUrl: Dispatch<SetStateAction<string>>;
  settingsCategory: SettingsCategory;
  setToolInput: Dispatch<SetStateAction<string>>;
  setWebSearchDraft: Dispatch<SetStateAction<WebSearchDraft>>;
  showSettingsSaved: (message?: string) => void;
  showTimelineView: () => void;
  sidecarBusy: boolean;
  sidecarDraft: SidecarDraft;
  sidecarState: SidecarState | null;
  skillBusy: boolean;
  skillInstallError: string | null;
  skillPackageInputRef: RefObject<HTMLInputElement>;
  skillRefreshTurn: number;
  skillState: SkillState | null;
  skillUrl: string;
  toolBusy: boolean;
  toolInput: string;
  updatePersonalizationDraft: (next: PersonalizationConfig) => void;
  webSearchBusy: boolean;
  webSearchConfig: WebSearchConfigState | null;
  webSearchDraft: WebSearchDraft;
  webSearchError: string | null;
  workspaceBusy: boolean;
  workspaceDraft: string;
  workspacePickerBusy: boolean;
};

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "local";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(timestampMs);
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
  return <Settings aria-hidden="true" />;
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

export function SettingsPage(props: SettingsPageProps) {
  const {
    activeRagOperation,
    canUseConfiguredKey,
    activePermissionReviews,
    appearanceMode,
    archivedSessions,
    browserBusy,
    browserTarget,
    browserText,
    browserUrl,
    busySessionIds,
    modelProfileCount,
    contextBusy,
    contextCheckpoint,
    debugAlwaysVisible,
    flushPersonalization,
    handleAddMcpServer,
    handleAnswerWithRag,
    handleCancelRag,
    handleAppearanceModeChange,
    handleCompactContext,
    handleIgnorePermissionReview,
    handleIndexRag,
    handleInstallSkillPackage,
    handleInstallSkillUrl,
    handleLoadProviderModels,
    handleReloadProviderState,
    handleMcpPolicy,
    handlePickWorkspace,
    handlePromptEvolutionToggle,
    handleRefreshMcpServer,
    handleRefreshSkills,
    handleRemoveMcpServer,
    handleResolvePermissionReview,
    handleRestorePermissionReview,
    handleRestoreSession,
    handleRunBrowserTool,
    handleRunTool,
    handleSavePersonalization,
    handleSaveProviderConfig,
    handleSaveSidecars,
    handleSaveWebSearch,
    handleSaveWorkspace,
    handleSearchRag,
    handleSkillPreference,
    ignoredPermissionReviews,
    imageEndpointValidation,
    knowledgeError,
    knowledgeGraphOpen,
    mcpBusy,
    mcpDraft,
    mcpState,
    permissionBusy,
    personalizationBusy,
    personalizationDraft,
    personalizationError,
    phase4,
    phase5,
    phase7,
    projectSessionBusy,
    projectSessionState,
    providerBusy,
    providerDraft,
    providerModelOptions,
    providerModels,
    providerModelsBusy,
    providerModelsError,
    providerModelsRefreshTurn,
    providerSettingsError,
    ragBusy,
    ragCancelling,
    ragProgress,
    ragQuery,
    ragSources,
    ragStats,
    runtime,
    selectedTool,
    selectedToolSpec,
    setBrowserTarget,
    setBrowserText,
    setBrowserUrl,
    setDebugAlwaysVisible,
    setKnowledgeGraphOpen,
    setMcpDraft,
    setProviderDraft,
    setRagQuery,
    setSelectedTool,
    setSettingsCategory,
    setSidecarDraft,
    setSkillUrl,
    settingsCategory,
    setToolInput,
    setWebSearchDraft,
    showSettingsSaved,
    showTimelineView,
    sidecarBusy,
    sidecarDraft,
    sidecarState,
    skillBusy,
    skillInstallError,
    skillPackageInputRef,
    skillRefreshTurn,
    skillState,
    skillUrl,
    toolBusy,
    toolInput,
    updatePersonalizationDraft,
    webSearchBusy,
    webSearchConfig,
    webSearchDraft,
    webSearchError,
    workspaceBusy,
    workspaceDraft,
    workspacePickerBusy
  } = props;
  const memorySettings = useMemorySettingsController({
    enabled: settingsCategory === "knowledge",
    projectId: projectSessionState?.activeProjectId || null,
    showSaved: showSettingsSaved
  });

  return (
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

                {settingsCategory === "knowledge" && (
                  <SettingsMemoryPanel
                    key={projectSessionState?.activeProjectId || "no-project"}
                    {...memorySettings}
                  />
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
                  <SettingsModelsPanel
                    canUseConfiguredKey={canUseConfiguredKey}
                    modelProfileCount={modelProfileCount}
                    handleLoadProviderModels={handleLoadProviderModels}
                    handleReloadProviderState={handleReloadProviderState}
                    handlePromptEvolutionToggle={handlePromptEvolutionToggle}
                    handleSaveProviderConfig={handleSaveProviderConfig}
                    imageEndpointValidation={imageEndpointValidation}
                    phase4={phase4}
                    providerBusy={providerBusy}
                    providerDraft={providerDraft}
                    providerModelOptions={providerModelOptions}
                    providerModels={providerModels}
                    providerModelsBusy={providerModelsBusy}
                    providerModelsError={providerModelsError}
                    providerModelsRefreshTurn={providerModelsRefreshTurn}
                    providerSettingsError={providerSettingsError}
                    setProviderDraft={setProviderDraft}
                  />
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
                  <SettingsPermissionsPanel
                    activeReviews={activePermissionReviews}
                    busy={permissionBusy}
                    busySessionIds={busySessionIds}
                    ignoredReviews={ignoredPermissionReviews}
                    onIgnore={handleIgnorePermissionReview}
                    onResolve={handleResolvePermissionReview}
                    onRestore={handleRestorePermissionReview}
                  />
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
                  <button
                    className="secondary-button"
                    type="button"
                    disabled={ragBusy}
                    onClick={handleIndexRag}
                  >
                    <Database size={17} aria-hidden="true" />
                    <span>
                      {activeRagOperation?.kind === "index"
                        ? "Indexing"
                        : ragBusy
                          ? "Working"
                          : "Index workspace"}
                    </span>
                  </button>
                  {activeRagOperation && ragBusy && (
                    <div className="rag-operation-status">
                      <div role="status" aria-live="polite" aria-atomic="true">
                        <strong>
                          {activeRagOperation.kind === "index"
                            ? "Indexing workspace"
                            : activeRagOperation.kind === "search"
                              ? "Searching workspace"
                              : "Answering with workspace knowledge"}
                        </strong>
                        <span>
                          {ragCancelling
                            ? "Cancelling..."
                            : ragProgress?.detail || "Starting..."}
                          {ragProgress && ragProgress.totalSteps > 0
                            ? ` · ${ragProgress.completedSteps}/${ragProgress.totalSteps}`
                            : ""}
                        </span>
                      </div>
                      <button
                        className="secondary-button"
                        type="button"
                        disabled={
                          ragCancelling || !ragProgress || ragProgress.status !== "running"
                        }
                        onClick={handleCancelRag}
                      >
                        Cancel
                      </button>
                    </div>
                  )}
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
                        {ragBusy && (phase7?.graph.nodes.length ?? 0) === 0 ? (
                          <div className="knowledge-graph-empty">Indexing workspace text...</div>
                        ) : (
                          <KnowledgeGraph
                            indexedAtMs={ragStats.indexedAtMs}
                            graph={
                              phase7?.graph ?? {
                                totalNodes: 0,
                                totalEdges: 0,
                                nodes: [],
                                edges: []
                              }
                            }
                          />
                        )}
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
                  <SettingsToolsPanel
                    browserBusy={browserBusy}
                    browserTarget={browserTarget}
                    browserText={browserText}
                    browserUrl={browserUrl}
                    onRunBrowserTool={handleRunBrowserTool}
                    onRunTool={handleRunTool}
                    onSaveWebSearch={handleSaveWebSearch}
                    phase5={phase5}
                    runtime={runtime}
                    selectedTool={selectedTool}
                    selectedToolSpec={selectedToolSpec}
                    setBrowserTarget={setBrowserTarget}
                    setBrowserText={setBrowserText}
                    setBrowserUrl={setBrowserUrl}
                    setSelectedTool={setSelectedTool}
                    setToolInput={setToolInput}
                    setWebSearchDraft={setWebSearchDraft}
                    sidecarState={sidecarState}
                    toolBusy={toolBusy}
                    toolInput={toolInput}
                    webSearchBusy={webSearchBusy}
                    webSearchConfig={webSearchConfig}
                    webSearchDraft={webSearchDraft}
                    webSearchError={webSearchError}
                  />
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
  );
}
