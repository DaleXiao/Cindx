import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties
} from "react";
import { CheckCircle2 } from "lucide-react";
import { Inspector } from "./components/Inspector";
import { Composer } from "./components/Composer";
import { PlanConfirmationCard } from "./components/PlanConfirmationCard";
import { QueuedMessages } from "./components/QueuedMessages";
import { Sidebar } from "./components/Sidebar";
import { WorkspaceChrome } from "./components/WorkspaceChrome";
import { providerReadinessMessage, providerStatusText } from "./providerReadinessModel";
import { useAgentRunController } from "./controllers/useAgentRunController";
import { useAppWorkspaceProjection } from "./controllers/useAppWorkspaceProjection";
import { useAppShellController } from "./controllers/useAppShellController";
import { useComposerAttachments } from "./controllers/useComposerAttachments";
import { useComposerDrafts } from "./controllers/useComposerDrafts";
import { usePreferencesController } from "./controllers/usePreferencesController";
import { useProviderSettingsController } from "./controllers/useProviderSettingsController";
import { useIntegrationSettingsController } from "./controllers/useIntegrationSettingsController";
import { useKnowledgeToolingController } from "./controllers/useKnowledgeToolingController";
import { useLatestAsyncSelection } from "./controllers/useLatestAsyncSelection";
import { usePermissionReviewController } from "./controllers/usePermissionReviewController";
import { useProjectSessionController } from "./controllers/useProjectSessionController";
import { useSessionRuntimeController } from "./controllers/useSessionRuntimeController";
import { useSessionRuntimeSync } from "./controllers/useSessionRuntimeSync";
import { useSidebarResize } from "./controllers/useSidebarResize";
import { LiveSessionThread } from "./components/SessionThread";
import {
  getAgentState,
  getProjectSessionState,
  getRuntimeStatus,
  revealMainWindow,
  resolvePermission,
  setSidebarMaterialWidth
} from "./tauri";
import type { PermissionReviewItem, ProjectSessionState, TimelineEntry } from "./tauri";

const SIDEBAR_MATERIAL_HIDE_DELAY_MS = 220;

// Stable empty list so the memoized thread does not re-render on unrelated
// state changes when no session is active.
const EMPTY_THREAD_TIMELINE: TimelineEntry[] = [];

const ScheduleView = lazy(() =>
  import("./components/ScheduleView").then((module) => ({
    default: module.ScheduleView
  }))
);

const SettingsPage = lazy(() =>
  import("./components/SettingsPage").then((module) => ({
    default: module.SettingsPage
  }))
);

export function App() {
  const [composerError, setComposerError] = useState<string | null>(null);
  const reportComposerError = useCallback(
    (message: string | null) => setComposerError(message),
    []
  );
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [newProjectName, setNewProjectName] = useState("");
  const [projectCreateOpen, setProjectCreateOpen] = useState(false);
  const [sidebarSearchOpen, setSidebarSearchOpen] = useState(false);
  const [sidebarQuery, setSidebarQuery] = useState("");
  const startupWindowRevealRequestedRef = useRef(false);
  const {
    activeView,
    debugAlwaysVisible,
    handleWorkspaceViewChange,
    inspectorOpen,
    inspectorOutputRequest,
    inspectorResizing,
    inspectorTab,
    inspectorWidth,
    openSettingsCategory,
    selectedScheduleId,
    setDebugAlwaysVisible,
    setInspectorOpen,
    setInspectorResizing,
    setInspectorTab,
    setInspectorWidth,
    setSelectedScheduleId,
    setSettingsCategory,
    setSidebarOpen,
    settingsCategory,
    showInspector,
    showInspectorOutput,
    showTimelineView,
    sidebarOpen
  } = useAppShellController();
  const {
    beginResize: beginSidebarResize,
    handleResizeKeyDown: handleSidebarResizeKeyDown,
    resizing: sidebarResizing,
    width: sidebarWidth
  } = useSidebarResize();
  const {
    appearanceMode,
    flushPersonalization,
    handleAppearanceModeChange,
    handleSavePersonalization,
    loadPersonalization,
    personalizationBusy,
    personalizationDraft,
    personalizationError,
    settingsToast,
    showSettingsSaved,
    updatePersonalizationDraft
  } = usePreferencesController();
  const {
    activeReviews: activePermissionReviews,
    ignoreReview: handleIgnorePermissionReview,
    ignoredReviews: ignoredPermissionReviews,
    refresh: refreshPermissionReviews,
    restoreReview: handleRestorePermissionReview
  } = usePermissionReviewController(
    activeView === "settings" && settingsCategory === "permissions"
  );
  const {
    activeRagOperation, browserApprovals,
    browserBusy,
    browserObservations,
    browserTarget,
    browserText,
    browserUrl,
    contextBusy,
    contextCheckpoint,
    contextState,
    handleAnswerWithRag, handleCancelRag,
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
    ragBusy, ragCancelling, ragProgress,
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
  } = useKnowledgeToolingController({
    reportError: reportComposerError,
    showInspector
  });
  const sessionRuntime = useSessionRuntimeController({
    setComposerError,
    setContextState,
    refreshPermissionReviews,
    setInspectorTab,
    setInspectorOpen
  });
  const {
    activeSessionIdRef,
    agentState,
    agentTraceState,
    applyAgentStateForSession,
    applyBootstrapAgentState,
    applyBootstrapProjectSessionState,
    busySessionIds,
    handleAgentStreamDone,
    handleExportAgentTrace,
    handleSessionEffortChange,
    handleSessionModelChange,
    handleThreadSelection,
    loadOlderAgentHistory,
    loadingOlderSessionId,
    optimisticUserMessageRevision,
    optimisticUserMessagesRef,
    projectSessionState,
    selectedThreadItem,
    selectedTraceStepId,
    sessionLoadingId,
    setSelectedThreadItem,
    setSelectedTraceStepId,
    streamResetVersion,
    traceBusy
  } = sessionRuntime;
  const {
    activeAgentState,
    activeProject,
    activeSession,
    activeSessionBusy,
    activeSessionTraceSteps,
    agentEffort,
    archivedSessions,
    projects,
    selectedTraceStep,
    sessionPrefetchKey,
    sessions,
    visibleAgentMessages
  } = useAppWorkspaceProjection({
    activeView,
    agentState,
    agentTraceState,
    busySessionIds,
    optimisticUserMessages: optimisticUserMessagesRef.current,
    optimisticUserMessageRevision,
    projectSessionState,
    selectedTraceStepId,
    sidebarQuery
  });
  const {
    value: composerDraft,
    focusRequest: composerFocusRequest,
    setActiveDraft: setActiveComposerDraft,
    appendDraftForSession: appendComposerDraftForSession,
    editActiveDraft: handleThreadMessageEdit,
    restoreDraftIfEmpty,
    forgetDrafts
  } = useComposerDrafts(activeSession?.id ?? null, activeSessionIdRef);
  const {
    attachments: composerAttachments,
    busy: attachmentBusy,
    clear: clearAttachments,
    forget: forgetAttachments,
    pick: handlePickAttachments,
    remove: handleRemoveAttachment,
    restoreIfEmpty: restoreAttachmentsIfEmpty
  } = useComposerAttachments({
    reportError: reportComposerError,
    sessionId: activeSession?.id ?? null
  });
  const enqueueProjectSessionSelection = useLatestAsyncSelection<ProjectSessionState>();
  const {
    completeBulkDelete,
    handleArchiveSession,
    handleCreateProject,
    handleCreateSession,
    handleDeleteProject,
    handleDeleteSession,
    handleForkSession,
    handleOpenScheduledSession,
    handlePickWorkspace,
    handleRenameProject,
    handleRenameSession,
    handleRestoreSession,
    handleSaveWorkspace,
    handleSelectProject,
    handleSelectSession,
    projectSessionBusy,
    runtime,
    setRuntime,
    setWorkspaceDraft,
    workspaceBusy,
    workspaceDraft,
    workspacePickerBusy
  } = useProjectSessionController({
    sessionRuntime,
    activeAgentState,
    activeProject,
    activeView,
    enqueueProjectSessionSelection,
    forgetAttachments,
    forgetDrafts,
    newProjectName,
    refreshWorkspaceKnowledge,
    setComposerError,
    setContextState,
    setNewProjectName,
    setProjectCreateOpen,
    showSettingsSaved,
    showTimelineView
  });
  const {
    canUseConfiguredKey,
    modelProfileCount,
    handleLoadProviderModels,
    handleSaveProviderConfig,
    imageEndpointValidation,
    loadProviderState,
    phase4,
    providerBusy,
    providerDraft,
    providerModelOptions,
    providerModels,
    providerModelsBusy,
    providerModelsError,
    providerModelsRefreshTurn,
    providerReadiness,
    providerSettingsError,
    setProviderDraft,
    voiceConfigured,
    voiceTransport
  } = useProviderSettingsController({
    runtimeProviderReady: runtime?.providerReady ?? null,
    showSaved: showSettingsSaved
  });
  const {
    handleAddMcpServer, handleImportExternalMcpServers,
    handleInstallSkillPackage,
    handleInstallSkillUrl,
    handleMcpPolicy,
    handleRefreshMcpServer,
    handleRefreshSkills,
    handleRemoveMcpServer,
    handleSaveSidecars,
    handleSaveWebSearch,
    handleSkillPreference,
    loadIntegrationState,
    mcpBusy,
    mcpDraft,
    mcpState,
    setMcpDraft,
    setSidecarDraft,
    setSkillUrl,
    setWebSearchDraft,
    sidecarBusy,
    sidecarDraft,
    sidecarState,
    skillBusy,
    skillInstallError,
    skillPackageInputRef,
    skillRefreshTurn,
    skillState,
    skillUrl,
    webSearchBusy,
    webSearchConfig,
    webSearchDraft,
    webSearchError
  } = useIntegrationSettingsController({
    reportError: reportComposerError,
    showSaved: showSettingsSaved
  });
  const {
    handleCancelAgentTask,
    handleDeleteQueuedMessage,
    handleEditQueuedMessage,
    handleResolveAgentPermission,
    handleRetryAgentTask,
    handleSendPrompt,
    handleSteerQueuedMessage,
    persistingQueuedMessageIds,
    queuedMessageBusyId
  } = useAgentRunController({
    sessionRuntime,
    activeAgentState,
    activeSession,
    agentEffort,
    attachmentBusy,
    clearAttachments,
    composerAttachments,
    refreshPermissionReviews,
    restoreAttachmentsIfEmpty,
    restoreDraftIfEmpty,
    runtime,
    setComposerError
  });
  useSessionRuntimeSync({
    sessionRuntime,
    activeAgentState,
    activeSession,
    activeSessionBusy,
    activeView,
    inspectorOpen,
    sessionPrefetchKey,
    setComposerError,
    setContextState
  });

  const handleArtifactInspect = useCallback(
    (path: string) => {
      const sessionId = activeSession?.id;
      if (!sessionId) return;
      showInspectorOutput({ sessionId, path, nonce: Date.now() });
    },
    [activeSession?.id, showInspectorOutput]
  );

  const statusText = useMemo(() => {
    if (!runtime) return "Connecting";
    if (runtime.kernelStatus === "kernel bridge online") return providerStatusText(providerReadiness);
    return runtime.kernelStatus;
  }, [providerReadiness, runtime]);

  const agentApprovals = activeAgentState?.pendingApprovals ?? [];
  const agentCanCancel = Boolean(activeAgentState?.canCancel || activeSessionBusy);
  const agentCanRetry = Boolean(activeAgentState?.canRetry);
  const pendingPlanConfirmation = activeAgentState?.pendingPlanConfirmation ?? null;
  const agentCanContinue = Boolean(activeAgentState?.canContinue) && !pendingPlanConfirmation;
  const agentWorking = Boolean(activeSessionBusy || activeAgentState?.status === "running");

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

  useEffect(() => {
    let frame: number | null = null;
    let timer: number | null = null;
    const width = activeView === "settings" || !sidebarOpen ? 0 : sidebarWidth;
    const updateMaterial = () => {
      frame = window.requestAnimationFrame(() => {
        void setSidebarMaterialWidth(width).catch(() => {});
      });
    };

    if (width === 0) {
      timer = window.setTimeout(updateMaterial, SIDEBAR_MATERIAL_HIDE_DELAY_MS);
    } else {
      updateMaterial();
    }

    return () => {
      if (timer !== null) window.clearTimeout(timer);
      if (frame !== null) window.cancelAnimationFrame(frame);
    };
  }, [activeView, sidebarOpen, sidebarWidth]);

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
        applyBootstrapProjectSessionState(state);
      }),
      getAgentState().then((state) => {
        applyBootstrapAgentState(state);
      }),
      loadPersonalization()
    ];

    const loadDeferredState = () => {
      if (disposed) return;
      loadIntegrationState();
      void refreshPermissionReviews().catch(() => {});
      void loadProviderState();
      loadKnowledgeState();
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
      } catch {}
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
      // Timers fire while the window is hidden (rAF does not), so this never blocks reveal.
      await new Promise<void>((resolve) => window.setTimeout(resolve, 80));
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
      <WorkspaceChrome
        activeView={activeView}
        contextEstimated={activeAgentState?.contextUsageEstimated ?? false}
        contextRemainingPercent={activeAgentState?.contextRemainingPercent ?? 100}
        contextTokensUsed={activeAgentState?.contextTokensUsed ?? 0}
        contextWindowTokens={
          activeAgentState?.contextWindowTokens ??
          phase4?.provider.contextWindowTokens ??
          128000
        }
        inspectorOpen={inspectorOpen}
        onInspectorToggle={() => setInspectorOpen((open) => !open)}
        onSidebarToggle={() => setSidebarOpen((open) => !open)}
        sidebarOpen={sidebarOpen}
        statusText={statusText}
        title={
          activeView === "settings"
            ? "Settings"
            : activeView === "schedule"
              ? "Schedule"
              : activeSession?.name ?? "Session"
        }
      />

      <div className="settings-transition-backdrop" aria-hidden="true" />

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
          onKeyDown={handleSidebarResizeKeyDown}
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
              timeline={activeAgentState?.timeline ?? EMPTY_THREAD_TIMELINE}
              streamResetVersion={streamResetVersion}
              status={activeAgentState?.status ?? "idle"}
              runStartedAtMs={activeAgentState?.runStartedAtMs ?? 0}
              hasOlderHistory={activeAgentState?.hasOlderHistory ?? false}
              loadingOlderHistory={loadingOlderSessionId === activeSession?.id}
              selectedId={selectedThreadItem?.id ?? null}
              onLoadOlderHistory={loadOlderAgentHistory}
              onSelect={handleThreadSelection}
              onEditMessage={handleThreadMessageEdit}
              onArtifactInspect={handleArtifactInspect}
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
              {pendingPlanConfirmation && activeSession?.id && (
                <PlanConfirmationCard sessionId={activeSession.id} confirmation={pendingPlanConfirmation} onResolved={applyAgentStateForSession} onError={setComposerError} />
              )}
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
                agentModel={activeSession?.agentModel ?? ""}
                modelOptions={
                  (phase4?.provider.enabledModels?.length ?? 0) > 0
                    ? phase4!.provider.enabledModels
                    : providerModelOptions.chat
                }
                sessionId={activeSession?.id ?? null}
                voiceConfigured={voiceConfigured}
                voiceTransport={voiceTransport}
                providerReadiness={providerReadiness}
                onChange={setActiveComposerDraft}
                onEffortChange={(effort) => void handleSessionEffortChange(effort)}
                onModelChange={(model) => void handleSessionModelChange(model)}
                onVoiceTranscript={appendComposerDraftForSession}
                onVoiceError={(message) => setComposerError(message)}
                onProviderRequired={(readiness) => setComposerError(providerReadinessMessage(readiness))}
                onConfigureProvider={() => {
                  openSettingsCategory("models");
                  void loadProviderState();
                }}
                onSend={(value, planMode) => void handleSendPrompt(value, planMode)}
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
          <Suspense fallback={<section className="settings-view" aria-busy="true" />}>
            <SettingsPage
            {...{
              activeRagOperation, activePermissionReviews,
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
              handleAnswerWithRag, handleCancelRag,
              handleAppearanceModeChange,
              handleCompactContext,
              handleIgnorePermissionReview,
              handleIndexRag,
              handleInstallSkillPackage,
              handleInstallSkillUrl,
              handleLoadProviderModels,
              handleReloadProviderState: loadProviderState,
              handleMcpPolicy, handleImportExternalMcpServers,
              handlePickWorkspace,
              handleRefreshMcpServer,
              handleRefreshSkills,
              handleRemoveMcpServer,
              handleResolvePermissionReview,
               handleRestorePermissionReview, handleRestoreSession,
               handleDeleteSession, completeBulkDelete,
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
              canUseConfiguredKey,
              ragBusy, ragCancelling, ragProgress,
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
            }}
            />
          </Suspense>
        )}
      </section>

      <Inspector
        open={activeView === "timeline" && inspectorOpen}
        showDebug={debugAlwaysVisible}
        width={inspectorWidth}
        tab={inspectorTab}
        sessionId={activeSession?.id ?? null}
        outputRequest={inspectorOutputRequest}
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
        agentMaxTurns={activeAgentState?.maxTurns ?? 0}
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
        onReview={() => {
          openSettingsCategory("permissions");
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
