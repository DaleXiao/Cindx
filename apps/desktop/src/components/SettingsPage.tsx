import {
  Activity,
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
  Dna,
  EyeOff,
  FileText,
  FolderOpen,
  Globe2,
  KeyRound,
  LayoutDashboard,
  Link2,
  Monitor,
  Moon,
  PackagePlus,
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
  type RagSourceView,
  type RagStatsView,
  type RuntimeStatus,
  type SessionView,
  type SidecarState,
  type SkillState,
  type ToolSpecView,
  type WebSearchConfigState
} from "../tauri";

export const DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible";

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

type WebSearchDraft = {
  endpoint: string;
  apiKey: string;
};

export type SettingsPageProps = {
  activePermissionReviews: PermissionReviewItem[];
  appearanceMode: AppearanceMode;
  archivedSessions: SessionView[];
  browserBusy: boolean;
  browserTarget: string;
  browserText: string;
  browserUrl: string;
  busySessionIds: Set<string>;
  collaborationModelCount: number;
  contextBusy: boolean;
  contextCheckpoint: ContextCheckpointView | null;
  debugAlwaysVisible: boolean;
  flushPersonalization: (notify: boolean) => Promise<void>;
  handleAddMcpServer: () => Promise<void>;
  handleAnswerWithRag: () => Promise<void>;
  handleAppearanceModeChange: (mode: AppearanceMode) => void;
  handleCompactContext: () => Promise<void>;
  handleIgnorePermissionReview: (requestId: string) => void;
  handleIndexRag: () => Promise<void>;
  handleInstallSkillPackage: (file: File) => Promise<void>;
  handleInstallSkillUrl: () => Promise<void>;
  handleLoadProviderModels: () => Promise<void>;
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
  providerModelOptions: string[];
  providerModels: string[];
  providerModelsBusy: boolean;
  providerModelsError: string | null;
  providerModelsRefreshTurn: number;
  ragBusy: boolean;
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
  setKnowledgeGraphOpen: Dispatch<SetStateAction<boolean>>;
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
  if (!effort.applicable) return "Not applicable";
  if (effort.evaluationInflight) return "Evaluating";
  if (effort.rolloutStatus === "canary") return `Canary ${effort.canaryPercent}%`;
  if (effort.rolloutStatus === "evaluating") return "Gathering evidence";
  if (effort.rolloutStatus === "rolled_back") return "Rolled back";
  if (effort.rolloutStatus === "promoted") return "Promoted";
  if (effort.status === "disabled") return "Off";
  return "Stable";
}

function promptEvolutionReadinessLabel(
  effort: Phase4State["promptEvolution"]["efforts"][number]
) {
  switch (effort.readiness) {
    case "not_applicable":
      return "Single-model path";
    case "disabled":
      return "Enable evolution";
    case "evaluating":
      return "Running offline pair";
    case "collecting_dataset":
      return `Need ${Math.max(0, 3 - effort.datasetCases)} completed task${Math.max(0, 3 - effort.datasetCases) === 1 ? "" : "s"}`;
    case "collecting_train_evidence":
      return "Collect train evidence";
    case "collecting_holdout_evidence":
      return "Collect holdout evidence";
    case "selecting_frontier":
      return "Select Pareto frontier";
    case "canary":
      return "Measure canary";
    case "rolled_back":
      return "Explore after rollback";
    case "promoted":
      return "Monitor promoted profile";
    default:
      return effort.nextMode.replace(/_/g, " ");
  }
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
    activePermissionReviews,
    appearanceMode,
    archivedSessions,
    browserBusy,
    browserTarget,
    browserText,
    browserUrl,
    busySessionIds,
    collaborationModelCount,
    contextBusy,
    contextCheckpoint,
    debugAlwaysVisible,
    flushPersonalization,
    handleAddMcpServer,
    handleAnswerWithRag,
    handleAppearanceModeChange,
    handleCompactContext,
    handleIgnorePermissionReview,
    handleIndexRag,
    handleInstallSkillPackage,
    handleInstallSkillUrl,
    handleLoadProviderModels,
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
    ragBusy,
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
                    direct stable-versus-challenger Wilson gate controls staged canary rollout with
                    automatic rollback.
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
                            <td title={`${effort.datasetCases} offline cases (${effort.datasetTrainCases} train · ${effort.datasetHoldoutCases} holdout) · ${effort.reflectionPackets} feedback reflections · ${effort.learnedProfiles} learned profiles`}>
                              {effort.applicable
                                ? `${effort.pairedRuns}/${effort.requiredPairedRuns} · ${effort.replayRuns}/${effort.requiredReplayRuns} · R${effort.reflectionPackets}`
                                : "-"}
                            </td>
                            <td>{effort.championScore === null ? "-" : `${Math.round(effort.championScore * 100)}%`}</td>
                            <td>{effort.promotionConfidence === null ? "-" : `${Math.round(effort.promotionConfidence * 100)}%`}</td>
                            <td title={`Ready ${effort.readyProfiles} · Stagnant ${effort.stagnantGenerations}/3`}>
                              Gen {effort.evaluatedGenerations} · Ready {effort.readyProfiles}
                            </td>
                            <td>{effort.rollbackCount}</td>
                            <td title={effort.freezeReason ?? undefined}>{promptEvolutionReadinessLabel(effort)}</td>
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
                                  className="permission-session"
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
                                className="permission-deny"
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
  );
}
