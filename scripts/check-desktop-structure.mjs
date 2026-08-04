import fs from "node:fs";
import path from "node:path";
import ts from "../apps/desktop/node_modules/typescript/lib/typescript.js";
import {
  inspectDesktopIntegrationBoundary,
  rustCodeWithoutCommentsAndLiterals,
} from "./desktop-integration-boundary.mjs";

const root = process.cwd();

const read = (relativePath) =>
  fs.readFileSync(path.join(root, relativePath), "utf8");

const countIdentifierCalls = (source, fileName, identifier) => {
  const sourceFile = ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.Latest,
    true,
    fileName.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  );
  let count = 0;
  const visit = (node) => {
    if (
      ts.isCallExpression(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === identifier
    ) {
      count += 1;
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return count;
};

const readRustSourceTree = (sourceDirectory) =>
  fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const entryPath = path.join(sourceDirectory, entry.name);
      if (entry.isDirectory()) return [readRustSourceTree(entryPath)];
      if (!entry.name.endsWith(".rs")) return [];
      return [`// ${entryPath}\n${fs.readFileSync(entryPath, "utf8")}`];
    })
    .join("\n");

const readFrontendSourceTree = (sourceDirectory) =>
  fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const entryPath = path.join(sourceDirectory, entry.name);
      if (entry.isDirectory()) return [readFrontendSourceTree(entryPath)];
      if (!entry.name.endsWith(".ts") && !entry.name.endsWith(".tsx")) return [];
      return [`// ${entryPath}\n${fs.readFileSync(entryPath, "utf8")}`];
    })
    .join("\n");

const readRustCrateSource = (crateName) =>
  readRustSourceTree(path.join(root, "crates", crateName, "src"));

const listRustSourceFiles = (sourceDirectory) =>
  fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const entryPath = path.join(sourceDirectory, entry.name);
      if (entry.isDirectory()) return listRustSourceFiles(entryPath);
      if (!entry.name.endsWith(".rs")) return [];
      return [entryPath];
    });

const listFrontendSourceFiles = (sourceDirectory) =>
  fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const entryPath = path.join(sourceDirectory, entry.name);
      if (entry.isDirectory()) return listFrontendSourceFiles(entryPath);
      if (!entry.name.endsWith(".ts") && !entry.name.endsWith(".tsx")) return [];
      return [entryPath];
    });

const productionRustSource = (source) => {
  const testModuleIndex = source.search(/\n#\[cfg\(test\)\]\s*\nmod tests\s*\{/);
  return testModuleIndex >= 0 ? source.slice(0, testModuleIndex) : source;
};

const productionRustLineCount = (source) =>
  productionRustSource(source).split("\n").length;

const capturedNames = (source, pattern) =>
  [...source.matchAll(pattern)].map((match) => match[1]);

const uniqueSortedNames = (names) => [...new Set(names)].sort();

const namesMissingFrom = (expected, actual) => {
  const actualNames = new Set(actual);
  return expected.filter((name) => !actualNames.has(name));
};

const parseJson = (relativePath) => JSON.parse(read(relativePath));

const assert = (condition, message) => {
  if (!condition) {
    throw new Error(message);
  }
};

const packageJson = parseJson("apps/desktop/package.json");
const packageLock = parseJson("apps/desktop/package-lock.json");
const tauriConfig = parseJson("apps/desktop/src-tauri/tauri.conf.json");
const capability = parseJson("apps/desktop/src-tauri/capabilities/default.json");
const appSource = read("apps/desktop/src/App.tsx");
const settingsPageFileSource = read("apps/desktop/src/components/SettingsPage.tsx");
const settingsModelsPanelSource = read(
  "apps/desktop/src/components/SettingsModelsPanel.tsx"
);
const settingsMemoryPanelSource = read(
  "apps/desktop/src/components/SettingsMemoryPanel.tsx"
);
const memoryManagementModelSource = read(
  "apps/desktop/src/memoryManagementModel.ts"
);
const memorySettingsControllerSource = read(
  "apps/desktop/src/controllers/useMemorySettingsController.ts"
);
const providerModalityFieldsSource = read(
  "apps/desktop/src/components/ProviderModalityFields.tsx"
);
const settingsModelsImplementationSource = [
  settingsModelsPanelSource,
  providerModalityFieldsSource,
].join("\n");
const settingsPermissionsPanelSource = read(
  "apps/desktop/src/components/SettingsPermissionsPanel.tsx"
);
const settingsToolsPanelSource = read(
  "apps/desktop/src/components/SettingsToolsPanel.tsx"
);
const promptEvolutionPanelSource = read(
  "apps/desktop/src/components/PromptEvolutionPanel.tsx"
);
const settingsPageSource = [
  settingsPageFileSource,
  settingsModelsImplementationSource,
  settingsMemoryPanelSource,
  settingsPermissionsPanelSource,
  settingsToolsPanelSource,
  promptEvolutionPanelSource,
].join("\n");
const preferencesControllerSource = read(
  "apps/desktop/src/controllers/usePreferencesController.ts"
);
const settingsUiSource = `${appSource}\n${settingsPageSource}\n${preferencesControllerSource}`;
const sessionRuntimeModelSource = read(
  "apps/desktop/src/sessionRuntimeModel.ts"
);
const appShellModelSource = read("apps/desktop/src/appShellModel.ts");
const appShellStateModelSource = read("apps/desktop/src/appShellStateModel.ts");
const appShellControllerSource = read(
  "apps/desktop/src/controllers/useAppShellController.ts"
);
const appWorkspaceProjectionSource = read(
  "apps/desktop/src/controllers/useAppWorkspaceProjection.ts"
);
const composerAttachmentsSource = read(
  "apps/desktop/src/controllers/useComposerAttachments.ts"
);
const attachmentIpcSource = read("apps/desktop/src/attachmentIpc.ts");
const artifactImagePreviewHookSource = read(
  "apps/desktop/src/controllers/useArtifactImagePreview.ts"
);
const artifactImagePreviewCacheSource = read(
  "apps/desktop/src/utils/artifactImagePreviewCache.ts"
);
const composerDraftsSource = read(
  "apps/desktop/src/controllers/useComposerDrafts.ts"
);
const latestAsyncSelectionSource = read(
  "apps/desktop/src/controllers/useLatestAsyncSelection.ts"
);
const permissionReviewControllerSource = read(
  "apps/desktop/src/controllers/usePermissionReviewController.ts"
);
const sidebarResizeSource = read(
  "apps/desktop/src/controllers/useSidebarResize.ts"
);
const sessionThreadFileSource = read("apps/desktop/src/components/SessionThread.tsx");
const sessionThreadViewCacheSource = read(
  "apps/desktop/src/components/sessionThreadViewCache.ts"
);
const sessionThreadNavigationSource = read(
  "apps/desktop/src/components/SessionThreadNavigation.tsx"
);
const sessionThreadArtifactsSource = read(
  "apps/desktop/src/components/SessionThreadArtifacts.tsx"
);
const sessionToolChainSource = read(
  "apps/desktop/src/components/SessionToolChain.tsx"
);
const sessionMinimapSource = read(
  "apps/desktop/src/components/SessionMinimap.tsx"
);
const sessionMinimapInteractionSource = read(
  "apps/desktop/src/components/useSessionMinimapInteraction.ts"
);
const agentMarkdownSource = read("apps/desktop/src/components/AgentMarkdown.tsx");
const streamingMarkdownModelSource = read(
  "apps/desktop/src/components/streamingMarkdownModel.ts"
);
const streamingMarkdownTailBufferSource = read(
  "apps/desktop/src/components/streamingMarkdownTailBuffer.ts"
);
const modelStreamAccumulatorSource = read(
  "apps/desktop/src/components/modelStreamAccumulator.ts"
);
const modelStreamEventRouterSource = read(
  "apps/desktop/src/components/modelStreamEventRouter.ts"
);
const modelStreamSubscriptionSource = read(
  "apps/desktop/src/components/modelStreamSubscription.ts"
);
const modelStreamAnswerSource = read(
  "apps/desktop/src/components/useModelStreamAnswer.ts"
);
const streamingMarkdownDeferredTailSource = read(
  "apps/desktop/src/components/StreamingMarkdownDeferredTail.tsx"
);
const sessionThreadSource = [
  sessionThreadFileSource,
  sessionThreadNavigationSource,
  sessionThreadArtifactsSource,
  sessionToolChainSource,
  sessionMinimapSource,
  sessionMinimapInteractionSource,
  agentMarkdownSource,
].join("\n");
const workspaceChromeSource = read(
  "apps/desktop/src/components/WorkspaceChrome.tsx"
);
const sessionThreadProjectionSource = read(
  "apps/desktop/src/components/sessionThreadProjection.ts"
);
const mermaidDiagramSource = read(
  "apps/desktop/src/components/MermaidDiagram.tsx"
);
const markmapDiagramSource = read(
  "apps/desktop/src/components/MarkmapDiagram.tsx"
);
const diagramFullscreenSource = read(
  "apps/desktop/src/components/DiagramFullscreen.tsx"
);
const resolvedThemeSource = read(
  "apps/desktop/src/components/useResolvedTheme.ts"
);
const markdownDiagramModelSource = read(
  "apps/desktop/src/components/markdownDiagramModel.ts"
);
const composerSource = read("apps/desktop/src/components/Composer.tsx");
const providerReadinessModelSource = read("apps/desktop/src/providerReadinessModel.ts");
const voiceInputButtonSource = read(
  "apps/desktop/src/components/VoiceInputButton.tsx"
);
const voiceInputHookSource = read("apps/desktop/src/voice/useVoiceInput.ts");
const openAiVoiceInputHookSource = read(
  "apps/desktop/src/voice/useOpenAiVoiceInput.ts"
);
const alibabaVoiceInputHookSource = read(
  "apps/desktop/src/voice/useAlibabaVoiceInput.ts"
);
const voicePcmRuntimeSource = read(
  "apps/desktop/src/voice/voicePcmRuntime.ts"
);
const voicePcmSupportSource = read(
  "apps/desktop/src/voice/voicePcmSupport.ts"
);
const voiceInputImplementationSource = [
  voiceInputHookSource,
  openAiVoiceInputHookSource,
  alibabaVoiceInputHookSource,
  voicePcmRuntimeSource,
  voicePcmSupportSource,
].join("\n");
const voiceInputModelSource = read("apps/desktop/src/voice/voiceInputModel.ts");
const voiceWebRtcRuntimeSource = read(
  "apps/desktop/src/voice/voiceWebRtcRuntime.ts"
);
const microphoneInfoPlist = read("apps/desktop/src-tauri/Info.plist");
const queuedMessagesSource = read("apps/desktop/src/components/QueuedMessages.tsx");
const scheduleViewSource = read("apps/desktop/src/components/ScheduleView.tsx");
const inspectorSource = read("apps/desktop/src/components/Inspector.tsx");
const sidebarSource = read("apps/desktop/src/components/Sidebar.tsx");
const disclosureTriangleSource = read(
  "apps/desktop/src/components/DisclosureTriangle.tsx"
);
const knowledgeGraphSource = read(
  "apps/desktop/src/components/KnowledgeGraph.tsx"
);
const traceStatusIconSource = read("apps/desktop/src/components/TraceStatusIcon.tsx");
const artifactProjectionSource = read("crates/agent-application/src/artifacts.rs");
const styleEntry = read("apps/desktop/src/styles.css");
const styleModuleDirectory = path.join(root, "apps/desktop/src/styles");
const styleModuleEntries = [
  "foundation.css",
  "sidebar.css",
  "workspace.css",
  "thread.css",
  "composer.css",
  "trace.css",
  "schedule.css",
  "settings.css",
  "knowledge-settings.css",
  "memory-settings.css",
  "inspector.css",
  "dark.css",
];
const styles = [
  styleEntry,
  ...styleModuleEntries.map((entry) =>
    fs.readFileSync(path.join(styleModuleDirectory, entry), "utf8")
  ),
].join("\n");
const tauriBridgeImplementation = read("apps/desktop/src/tauri.ts");
const tauriTypesSource = read("apps/desktop/src/tauriTypes.ts");
const tauriNativeTypesSource = read("apps/desktop/src/tauriNativeTypes.ts");
const agentRunBudgetModelSource = read(
  "apps/desktop/src/agentRunBudgetModel.ts"
);
const tauriBridge = `${tauriBridgeImplementation}\n${tauriTypesSource}\n${tauriNativeTypesSource}\n${agentRunBudgetModelSource}`;
const desktopControllerEntries = [
  "useAppShellController.ts",
  "useAppWorkspaceProjection.ts",
  "useComposerAttachments.ts",
  "useComposerDrafts.ts",
  "useLatestAsyncSelection.ts",
  "usePermissionReviewController.ts",
  "useSidebarResize.ts",
  "usePreferencesController.ts",
  "useProviderSettingsController.ts",
  "useIntegrationSettingsController.ts",
  "useKnowledgeToolingController.ts",
];
const desktopControllers = desktopControllerEntries.map((entry) => ({
  entry,
  source: read(`apps/desktop/src/controllers/${entry}`),
}));
const desktopControllerSource = desktopControllers
  .map(({ source }) => source)
  .join("\n");
const desktopUiSource = `${appSource}\n${settingsPageSource}\n${desktopControllerSource}`;
const localBuildScript = read("scripts/build-local-app.mjs");
const browserSidecarSource = read("scripts/sidecars/browser-sidecar.js");
const browserIntegrationTest = read("scripts/test-browser-sidecar.mjs");
const computerSidecarSource = read("scripts/sidecars/computer-sidecar.js");
const computerIntegrationTest = read("scripts/test-computer-sidecar.mjs");
const desktopRustSourceDirectory = path.join(
  root,
  "apps/desktop/src-tauri/src"
);
const desktopFrontendSourceDirectory = path.join(root, "apps/desktop/src");
const desktopFrontendSource = readFrontendSourceTree(
  desktopFrontendSourceDirectory
);
const desktopRustModules = fs
  .readdirSync(desktopRustSourceDirectory)
  .filter((entry) => entry.endsWith(".rs"))
  .sort()
  .map((entry) => ({
    entry,
    source: fs.readFileSync(path.join(desktopRustSourceDirectory, entry), "utf8"),
  }));
const rustCompositionRoot = read("apps/desktop/src-tauri/src/lib.rs");
const rustLib = readRustSourceTree(desktopRustSourceDirectory);
const appBootstrapSource = read(
  "apps/desktop/src-tauri/src/app_bootstrap.rs"
);
const agentModelTurnRuntimeSource = read(
  "apps/desktop/src-tauri/src/agent_model_turn_runtime.rs"
);
const voiceCommandsSource = read(
  "apps/desktop/src-tauri/src/voice_commands.rs"
);
const desktopEventSinkSource = read(
  "apps/desktop/src-tauri/src/desktop_event_sink.rs"
);
const collaborationServiceSource = read(
  "apps/desktop/src-tauri/src/collaboration_service.rs"
);
const collaborationWorkerRuntimeSource = read(
  "apps/desktop/src-tauri/src/collaboration_worker_runtime.rs"
);
const adaptiveCollaborationFinalizationSource = read(
  "apps/desktop/src-tauri/src/adaptive_collaboration_finalization.rs"
);
const agentRuntimeModelTransportSource = read(
  "crates/agent-runtime/src/model_transport.rs"
);
const agentRuntimeGroundingPolicySource = read(
  "crates/agent-runtime/src/grounding_policy.rs"
);
const agentRuntimeGroundingToolsSource = read(
  "crates/agent-runtime/src/grounding_tools.rs"
);
const agentLoopContractRuntimeSource = read(
  "apps/desktop/src-tauri/src/agent_loop_contract_runtime.rs"
);
const desktopAgentToolRuntimeSource = read(
  "apps/desktop/src-tauri/src/agent_tool_runtime.rs"
);
const agentRecoveryServiceSource = read(
  "apps/desktop/src-tauri/src/agent_recovery_service.rs"
);
const agentConductorRuntimeSource = read(
  "apps/desktop/src-tauri/src/agent_conductor_runtime.rs"
);
const agentTaskCommandSource = read(
  "apps/desktop/src-tauri/src/agent_commands/task.rs"
);
const agentRuntimeSnapshotSource = read(
  "apps/desktop/src-tauri/src/agent_runtime_snapshot.rs"
);
const sessionOutputCacheSource = read(
  "apps/desktop/src-tauri/src/session_output_cache.rs"
);
const sessionOutputCacheStoreSource = read(
  "apps/desktop/src-tauri/src/session_output_cache_store.rs"
);
const memoryProjectionRuntimeSource = read(
  "apps/desktop/src-tauri/src/memory_projection_runtime.rs"
);
const promptEvolutionWorkerSource = read(
  "apps/desktop/src-tauri/src/prompt_evolution_worker.rs"
);
const promptEvolutionReadModelSource = read(
  "apps/desktop/src-tauri/src/prompt_evolution_read_model.rs"
);
const promptEvolutionRuntimeSource = read(
  "apps/desktop/src-tauri/src/prompt_evolution_runtime.rs"
);
const promptEvolutionHotStateSource = read(
  "apps/desktop/src-tauri/src/prompt_evolution_hot_state.rs"
);
const promptLearningOutboxProjectionSource = read(
  "apps/desktop/src-tauri/src/prompt_learning_outbox_projection.rs"
);
const promptPairwiseRuntimeSource = read(
  "apps/desktop/src-tauri/src/prompt_pairwise_runtime.rs"
);
const parallelExecutionSource = read(
  "apps/desktop/src-tauri/src/parallel_execution.rs"
);
const permissionServiceSource = read(
  "apps/desktop/src-tauri/src/permission_service.rs"
);
const agentCorePermissionPolicySource = read(
  "crates/agent-core/src/permission_policy.rs"
);
const queueServiceSource = read("apps/desktop/src-tauri/src/queue_service.rs");
const agentRunEngineSource = read(
  "apps/desktop/src-tauri/src/agent_run_engine.rs"
);
const agentCompletionRuntimeSource = read(
  "apps/desktop/src-tauri/src/agent_completion_runtime.rs"
);
const runExecutionSource = read("crates/agent-application/src/run_execution.rs");
const runLifecycleSource = read("crates/agent-application/src/run_lifecycle.rs");
const sessionProjectionSource = read(
  "apps/desktop/src-tauri/src/session_projection.rs"
);
const sessionContextServiceSource = read(
  "apps/desktop/src-tauri/src/session_context_service.rs"
);
const sessionTitleServiceSource = read(
  "apps/desktop/src-tauri/src/session_title_service.rs"
);
const toolRuntimeServiceSource = read(
  "apps/desktop/src-tauri/src/tool_runtime_service.rs"
);
const toolExecutionSource = read(
  "apps/desktop/src-tauri/src/tool_execution.rs"
);
const manualToolExecutionSource = read(
  "apps/desktop/src-tauri/src/manual_tool_execution.rs"
);
const appStateSource = read("apps/desktop/src-tauri/src/app_state.rs");
const toolCommandsSource = read("apps/desktop/src-tauri/src/tool_commands.rs");
const knowledgeCommandsSource = read(
  "apps/desktop/src-tauri/src/knowledge_commands.rs"
);
const scheduleSource = read("apps/desktop/src-tauri/src/schedule.rs");
const cargoToml = read("apps/desktop/src-tauri/Cargo.toml");
const cargoLock = read("apps/desktop/src-tauri/Cargo.lock");
const runTauriSource = read("scripts/run-tauri.mjs");
const stampBuildVersionSource = read("scripts/stamp-build-version.mjs");
const versioningSource = read("scripts/versioning.mjs");
const releaseWorkflow = read(".github/workflows/release.yml");
const unsignedReleaseStart = releaseWorkflow.indexOf(
  "- name: Build and publish unsigned Universal app"
);
const unsignedReleaseBlock =
  unsignedReleaseStart >= 0 ? releaseWorkflow.slice(unsignedReleaseStart) : "";
const ciWorkflow = read(".github/workflows/ci.yml");
const frontendCheckScript = read("scripts/check-frontend.sh");
const releaseVersionCheck = read("scripts/check-release-version.mjs");
const toolsSource = readRustCrateSource("tools");
const agentStorageSource = read("crates/agent-storage/src/lib.rs");
const agentSkillsSource = read("crates/agent-skills/src/lib.rs");
const builtinSkillCreator = read("crates/agent-skills/builtins/skill-creator/SKILL.md");
const modelProviderSource = readRustCrateSource("model-provider");
const dashScopeRealtimeProviderSource = read(
  "crates/model-provider/src/dashscope_realtime_provider.rs"
);
const dashScopeRealtimeGuardSource = read(
  "crates/model-provider/src/dashscope_realtime_guard.rs"
);
const dashScopeAsrTaskProviderSource = read(
  "crates/model-provider/src/dashscope_asr_task_provider.rs"
);
const modelProviderCargo = read("crates/model-provider/Cargo.toml");
const ragSource = read("crates/agent-rag/src/lib.rs");
const graphSource = read("crates/agent-graph/src/lib.rs");
const agentMemorySource = readRustCrateSource("agent-memory");
const agentRuntimeSource = readRustCrateSource("agent-runtime");
const agentToolRuntimeSource = read("crates/agent-runtime/src/tool_runtime.rs");
const runControlSource = read("crates/agent-runtime/src/control.rs");
const coreAgentPrompt = read("crates/agent-runtime/src/core_prompt.txt");
const orchestratorSource = readRustCrateSource("orchestrator");
const promptEvolutionSource = readRustCrateSource("orchestrator");
const benchmarkSource = read("crates/orchestrator-eval/src/benchmark.rs");
const benchmarkSuite = JSON.parse(read("benchmarks/agent/core-v1.json"));
const benchmarkBaseline = JSON.parse(read("benchmarks/agent/core-v1-baseline.json"));
const memoryBenchmarkSuite = JSON.parse(read("benchmarks/agent/memory-v1.json"));
const qualityGateManifest = JSON.parse(
  read("benchmarks/system/quality-gates-v1.json")
);
const desktopDtoContract = JSON.parse(
  read("apps/desktop/contracts/tauri-dto-v1.json")
);
const desktopDtoContractRunner = read("scripts/check-desktop-dto-contract.mjs");
const desktopDtoContractRustTest = read(
  "apps/desktop/src-tauri/src/view_model_contract_tests.rs"
);
const shippingPerformanceGateIds = [
  "session-projection-scaling",
  "agent-runtime-snapshot-scaling",
  "prompt-learning-outbox-scaling",
  "workspace-graph-cache-scaling",
  "prepared-image-request-scaling",
  "model-transport-prepare-scaling",
  "frontend-streaming-markdown-scaling"
];
const shippingPerformanceProofs = new Map([
  [
    "session-projection-scaling",
    [
      "session_projection::tests::incremental_projection_reads_only_the_target_session_delta",
      "cindx.session-projection-diagnostic.v1"
    ]
  ],
  [
    "agent-runtime-snapshot-scaling",
    [
      "agent_runtime_snapshot_cursor::tests::incremental_runtime_snapshot_visits_only_appended_messages",
      "cindx.agent-runtime-snapshot-scaling.v1"
    ]
  ],
  [
    "prompt-learning-outbox-scaling",
    [
      "prompt_learning_outbox_projection::tests::prompt_learning_outbox_delta_projection_scaling_gate",
      "cindx.prompt-learning-outbox-scaling.v1"
    ]
  ],
  [
    "workspace-graph-cache-scaling",
    [
      "tests::workspace_knowledge_snapshot_reuses_one_graph_parse_and_borrowed_projection",
      "cindx.workspace-graph-cache-scaling.v1"
    ]
  ],
  [
    "prepared-image-request-scaling",
    [
      "prepared_request::tests::prepared_streaming_body_matches_canonical_json_and_shares_bytes",
      "cindx.prepared-image-request-scaling.v1"
    ]
  ],
  [
    "model-transport-prepare-scaling",
    [
      "agent_model_turn_runtime::tests::transport_attempts_prepare_model_request_once",
      "cindx.model-transport-prepare-scaling.v1"
    ]
  ],
  [
    "frontend-streaming-markdown-scaling",
    [
      "keeps rope string work linear across an eight MiB unbroken line",
      "cindx.frontend-streaming-markdown-scaling.v1"
    ]
  ]
]);
const manualPerformanceProofs = new Map([
  [
    "context-governor-scaling",
    [
      "context_governor::tests::long_history_context_governor_scaling_diagnostic",
      "cindx.context-governor-diagnostic.v1"
    ]
  ],
  [
    "conductor-health-scaling",
    [
      "conductor_health_runtime::tests::conductor_health_scaling_diagnostic",
      "cindx.conductor-health-diagnostic.v1"
    ]
  ],
  [
    "rag-search-scaling",
    ["tests::synthetic_rag_search_scaling_diagnostic", "cindx.rag-search-diagnostic.v1"]
  ]
]);
const qualityGateById = new Map(
  qualityGateManifest.gates.map((gate) => [gate.id, gate])
);
const qualityGateRunner = read("scripts/run-quality-gates.mjs");
const qualityGateDoc = read("docs/QUALITY_GATES.md");
const evaluationLabSource = read(
  "crates/orchestrator-eval/examples/evaluation_lab.rs"
);
const shippingOrchestratorExamples = fs.existsSync(
  path.join(root, "crates", "orchestrator", "examples")
)
  ? fs.readdirSync(path.join(root, "crates", "orchestrator", "examples"))
  : [];
const memoryEvaluationLabSource = read("crates/agent-memory/examples/memory_lab.rs");
const agentEvaluationDoc = read("docs/AGENT_EVALUATION.md");
const browserControlDoc = read("docs/BROWSER_CONTROL.md");
const mainSource = read("apps/desktop/src/main.tsx");
const html = read("apps/desktop/index.html");
const viteConfig = read("apps/desktop/vite.config.ts");
const sessionRefreshStart = appSource.indexOf(
  "async function refreshWorkspaceAfterProjectSession"
);
const sessionRefreshEnd = appSource.indexOf(
  "function forgetDeletedSessions",
  sessionRefreshStart
);
const sessionRefreshBlock = appSource.slice(sessionRefreshStart, sessionRefreshEnd);

const frontendInvokeCommandNames = uniqueSortedNames(
  capturedNames(
    desktopFrontendSource,
    /\binvoke(?:<[^>]*>)?\(\s*["']([A-Za-z_][A-Za-z0-9_]*)["']/g
  )
);
const frontendInvokeOwnershipViolations = listFrontendSourceFiles(
  desktopFrontendSourceDirectory
)
  .filter(
    (file) =>
      !["tauri.ts", "attachmentIpc.ts"].includes(
        path.relative(desktopFrontendSourceDirectory, file)
      )
  )
  .filter((file) => /\binvoke(?:<|\()/.test(fs.readFileSync(file, "utf8")))
  .map((file) => path.relative(desktopFrontendSourceDirectory, file));
const rustCommandDefinitionNames = uniqueSortedNames(
  capturedNames(
    rustLib,
    /#\[tauri::command(?:\([^\]]*\))?\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/g
  )
);
const tauriHandlerBlocks = capturedNames(
  rustCodeWithoutCommentsAndLiterals(appBootstrapSource),
  /tauri::generate_handler!\s*\[([\s\S]*?)\]/g
);
const registeredTauriCommandPaths = uniqueSortedNames(
  tauriHandlerBlocks.flatMap((block) =>
    block
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean)
  )
);
const registeredTauriCommandNames = uniqueSortedNames(
  registeredTauriCommandPaths.map((entry) => entry.split("::").at(-1))
);
const frontendCommandsMissingDefinitions = namesMissingFrom(
  frontendInvokeCommandNames,
  rustCommandDefinitionNames
);
const rustCommandsMissingFrontendInvokes = namesMissingFrom(
  rustCommandDefinitionNames,
  frontendInvokeCommandNames
);
const definedCommandsMissingRegistration = namesMissingFrom(
  rustCommandDefinitionNames,
  registeredTauriCommandNames
);
const registeredCommandsMissingDefinitions = namesMissingFrom(
  registeredTauriCommandNames,
  rustCommandDefinitionNames
);

const rustCompositionRootLineCount = rustCompositionRoot.split("\n").length;
const appLineCount = appSource.split("\n").length;
const appUseStateCount = countIdentifierCalls(appSource, "App.tsx", "useState");
const appUseStateCounterProbe = countIdentifierCalls(
  `function Probe() {
    useState(0);
    useState<string>("");
    useState<Set<string>>(() => new Set());
  }`,
  "hook-counter-probe.tsx",
  "useState"
);
const settingsPageLineCount = settingsPageFileSource.split("\n").length;
const sessionThreadLineCount = sessionThreadFileSource.split("\n").length;
const inspectorLineCount = inspectorSource.split("\n").length;
const extractedDesktopBoundaryBudgets = [
  ["appShellModel.ts", appShellModelSource, 80],
  ["appShellStateModel.ts", appShellStateModelSource, 160],
  ["useAppShellController.ts", appShellControllerSource, 100],
  ["useAppWorkspaceProjection.ts", appWorkspaceProjectionSource, 200],
  ["useComposerAttachments.ts", composerAttachmentsSource, 140],
  ["attachmentIpc.ts", attachmentIpcSource, 80],
  ["useArtifactImagePreview.ts", artifactImagePreviewHookSource, 70],
  ["artifactImagePreviewCache.ts", artifactImagePreviewCacheSource, 120],
  ["useComposerDrafts.ts", composerDraftsSource, 90],
  ["useVoiceInput.ts", voiceInputHookSource, 280],
  ["useOpenAiVoiceInput.ts", openAiVoiceInputHookSource, 290],
  ["useAlibabaVoiceInput.ts", alibabaVoiceInputHookSource, 190],
  ["voicePcmRuntime.ts", voicePcmRuntimeSource, 200],
  ["voicePcmSupport.ts", voicePcmSupportSource, 100],
  ["voiceInputModel.ts", voiceInputModelSource, 120],
  ["voiceWebRtcRuntime.ts", voiceWebRtcRuntimeSource, 100],
  ["useLatestAsyncSelection.ts", latestAsyncSelectionSource, 80],
  ["usePermissionReviewController.ts", permissionReviewControllerSource, 130],
  ["useSidebarResize.ts", sidebarResizeSource, 80],
  ["AgentMarkdown.tsx", agentMarkdownSource, 340],
  ["streamingMarkdownModel.ts", streamingMarkdownModelSource, 180],
  ["streamingMarkdownTailBuffer.ts", streamingMarkdownTailBufferSource, 210],
  ["modelStreamAccumulator.ts", modelStreamAccumulatorSource, 200],
  ["modelStreamEventRouter.ts", modelStreamEventRouterSource, 170],
  ["modelStreamSubscription.ts", modelStreamSubscriptionSource, 120],
  ["useModelStreamAnswer.ts", modelStreamAnswerSource, 160],
  ["StreamingMarkdownDeferredTail.tsx", streamingMarkdownDeferredTailSource, 60],
  ["PromptEvolutionPanel.tsx", promptEvolutionPanelSource, 280],
  ["SettingsModelsPanel.tsx", settingsModelsPanelSource, 320],
  ["ProviderModalityFields.tsx", providerModalityFieldsSource, 140],
  ["VoiceInputButton.tsx", voiceInputButtonSource, 80],
  ["providerReadinessModel.ts", providerReadinessModelSource, 80],
  ["SettingsMemoryPanel.tsx", settingsMemoryPanelSource, 400],
  ["useMemorySettingsController.ts", memorySettingsControllerSource, 180],
  ["SettingsPermissionsPanel.tsx", settingsPermissionsPanelSource, 220],
  ["SettingsToolsPanel.tsx", settingsToolsPanelSource, 420],
  ["SessionThreadArtifacts.tsx", sessionThreadArtifactsSource, 260],
  ["SessionThreadNavigation.tsx", sessionThreadNavigationSource, 260],
  ["sessionThreadViewCache.ts", sessionThreadViewCacheSource, 160],
  ["SessionToolChain.tsx", sessionToolChainSource, 180],
  ["SessionMinimap.tsx", sessionMinimapSource, 130],
  ["useSessionMinimapInteraction.ts", sessionMinimapInteractionSource, 150],
  ["WorkspaceChrome.tsx", workspaceChromeSource, 150],
];
const oversizedExtractedDesktopBoundaries = extractedDesktopBoundaryBudgets.filter(
  ([, source, budget]) => source.split("\n").length > budget
);
const oversizedDesktopControllers = desktopControllers
  .map(({ entry, source }) => ({ entry, lines: source.split("\n").length }))
  .filter(({ lines }) => lines > 500);
const tauriBridgeImplementationLineCount = tauriBridgeImplementation.split("\n").length;
const tauriTypesLineCount = tauriTypesSource.split("\n").length;
const tauriNativeTypesLineCount = tauriNativeTypesSource.split("\n").length;
const unguardedTauriFallbacks = tauriBridgeImplementation
  .split("\n")
  .flatMap((line, index, lines) => {
    if (!/^  } catch \(error\) \{$/.test(line)) return [];
    const guard = lines[index + 1]?.trim() ?? "";
    return guard === "requireBrowserPreviewFallback(error);" ||
      guard === "if (isTauriRuntime()) throw error;"
      ? []
      : [index + 1];
  });
const browserWatchdogStart = browserSidecarSource.indexOf(
  "async function watchBrowserSession"
);
const browserWatchdogEnd = browserSidecarSource.indexOf(
  "function sessionLeaseExpired",
  browserWatchdogStart
);
const browserWatchdogBlock = browserSidecarSource.slice(
  browserWatchdogStart,
  browserWatchdogEnd
);
const oversizedStyleModules = styleModuleEntries
  .map((entry) => ({
    entry,
    lines: fs
      .readFileSync(path.join(styleModuleDirectory, entry), "utf8")
      .split("\n").length,
  }))
  .filter(({ lines }) => lines > 1_900);
const oversizedProductionRustModules = desktopRustModules
  .filter(
    ({ entry }) =>
      entry !== "tests.rs" &&
      !entry.endsWith("_tests.rs") &&
      !entry.endsWith("_eval_tests.rs")
  )
  .map(({ entry, source }) => ({ entry, lines: source.split("\n").length }))
  .filter(({ lines }) => lines > 1_200);
const criticalDesktopAgentModuleBudgets = new Map([
  ["attachment_commands.rs", 190],
  ["attachment_upload_batches.rs", 170],
  ["agent_run_engine.rs", 250],
  ["agent_conductor_runtime.rs", 140],
  ["agent_strategy_context.rs", 140],
  ["agent_strategy_runtime.rs", 420],
  ["agent_loop_runtime.rs", 550],
  ["agent_collaboration_runtime.rs", 800],
  ["agent_recovery_service.rs", 550],
  ["agent_runtime_snapshot.rs", 220],
  ["background_work_runtime.rs", 80],
  ["configuration_persistence.rs", 400],
  ["event_persistence.rs", 180],
  ["event_security.rs", 500],
  ["permission_service.rs", 220],
  ["project_session_persistence.rs", 600],
  ["prompt_evolution_worker.rs", 650],
  ["prompt_workflow_execution.rs", 800],
  ["runtime_values.rs", 420],
  ["session_context_service.rs", 550],
  ["session_output_cache.rs", 180],
  ["session_output_cache_store.rs", 130],
  ["sidecar_runtime.rs", 300],
  ["semantic_memory_runtime.rs", 260],
  ["semantic_memory_worker.rs", 240],
  ["knowledge_runtime.rs", 1_000],
  ["memory_projection_runtime.rs", 260],
  ["memory_runtime.rs", 950],
  ["manual_tool_execution.rs", 180],
]);
const criticalDesktopAgentModules = desktopRustModules.filter(({ entry }) =>
  criticalDesktopAgentModuleBudgets.has(entry)
);
const implicitCriticalDesktopAgentModules = criticalDesktopAgentModules.filter(
  ({ source }) => /^use super::\*;/m.test(source)
);
const desktopIntegrationBoundary = inspectDesktopIntegrationBoundary(root);
const oversizedCriticalDesktopAgentModules = criticalDesktopAgentModules
  .map(({ entry, source }) => ({
    entry,
    lines: source.split("\n").length,
    budget: criticalDesktopAgentModuleBudgets.get(entry),
  }))
  .filter(({ lines, budget }) => lines > budget);
const desktopAdapterModuleBudgets = new Map([
  ["desktop_event_sink.rs", 220],
]);
const desktopAdapterModules = desktopRustModules.filter(({ entry }) =>
  desktopAdapterModuleBudgets.has(entry)
);
const oversizedDesktopAdapterModules = desktopAdapterModules
  .map(({ entry, source }) => ({
    entry,
    lines: source.split("\n").length,
    budget: desktopAdapterModuleBudgets.get(entry),
  }))
  .filter(({ lines, budget }) => lines > budget);
const desktopEventOwnershipViolations = desktopRustModules
  .filter(
    ({ entry, source }) =>
      entry !== "desktop_event_sink.rs" &&
      (source.includes('"model-stream-delta"') ||
        source.includes('"session-title-updated"') ||
        source.includes("tauri::Emitter") ||
        source.includes(".emit("))
  )
  .map(({ entry }) => entry);
const oversizedAgentCoreModules = ["agent-runtime", "agent-memory", "orchestrator"]
  .flatMap((crateName) =>
    listRustSourceFiles(path.join(root, "crates", crateName, "src"))
      .filter((file) => {
        const name = path.basename(file);
        return name !== "tests.rs" && !name.endsWith("_tests.rs");
      })
      .map((file) => {
        const source = fs.readFileSync(file, "utf8");
        return {
          file: path.relative(root, file),
          lines: productionRustLineCount(source),
        };
      })
  )
  .filter(({ lines }) => lines > 1_450);
const modelProviderModuleBudgets = new Map([
  ["dashscope_asr_task_provider.rs", 260],
  ["dashscope_realtime_config.rs", 100],
  ["dashscope_realtime_guard.rs", 80],
  ["dashscope_realtime_provider.rs", 300],
  ["error.rs", 160],
  ["image_provider.rs", 430],
  ["json_wire.rs", 320],
  ["lib.rs", 900],
  ["prepared_payload.rs", 70],
  ["prepared_request.rs", 140],
  ["provider_receipt.rs", 150],
  ["provider_validation.rs", 220],
  ["realtime_provider.rs", 190],
  ["redirect_policy.rs", 100],
  ["request_builder.rs", 340],
  ["request_tool_calls.rs", 80],
  ["request_vision.rs", 400],
  ["response_parser.rs", 380],
  ["stream_delta_aggregator.rs", 150],
  ["streaming_finish.rs", 120],
  ["streaming_response.rs", 380],
  ["streaming_wire.rs", 160],
  ["usage.rs", 220],
]);
const modelProviderModules = listRustSourceFiles(
  path.join(root, "crates", "model-provider", "src")
).map((file) => ({
  entry: path.basename(file),
  lines: productionRustLineCount(fs.readFileSync(file, "utf8")),
}));
const oversizedModelProviderModules = modelProviderModules.filter(({ entry, lines }) => {
  const budget = modelProviderModuleBudgets.get(entry);
  return budget !== undefined && lines > budget;
});
const toolsModuleBudgets = new Map([
  ["browser_session_retirement.rs", 100],
  ["desktop_control.rs", 1_350],
  ["file_batch.rs", 230],
  ["file_search.rs", 260],
  ["file_tools.rs", 540],
  ["image_generation.rs", 280],
  ["lib.rs", 720],
  ["meta_tools.rs", 240],
  ["private_file.rs", 80],
  ["process_control.rs", 30],
  ["shell.rs", 950],
  ["stream_capture.rs", 60],
  ["tool_support.rs", 360],
  ["web_search.rs", 420],
]);
const toolsModules = listRustSourceFiles(
  path.join(root, "crates", "tools", "src")
).map((file) => {
  const source = fs.readFileSync(file, "utf8");
  return {
    entry: path.basename(file),
    lines: productionRustLineCount(source),
  };
});
const oversizedToolsModules = toolsModules.filter(({ entry, lines }) => {
  const budget = toolsModuleBudgets.get(entry);
  return budget !== undefined && lines > budget;
});

assert(
  rustCompositionRootLineCount <= 250 &&
    !rustCompositionRoot.includes("#[tauri::command]") &&
    !rustCompositionRoot.includes("pub(crate) fn "),
  `Desktop Rust composition root must remain declarative (found ${rustCompositionRootLineCount} lines)`
);
assert(
  desktopIntegrationBoundary.ok,
  desktopIntegrationBoundary.message
);
assert(
  tauriHandlerBlocks.length === 1 &&
    frontendInvokeOwnershipViolations.length === 0 &&
    frontendInvokeCommandNames.length > 0 &&
    rustCommandDefinitionNames.length > 0 &&
    registeredTauriCommandNames.length > 0 &&
    frontendCommandsMissingDefinitions.length === 0 &&
    rustCommandsMissingFrontendInvokes.length === 0 &&
    definedCommandsMissingRegistration.length === 0 &&
    registeredCommandsMissingDefinitions.length === 0,
  `Tauri command parity regressed: frontend=${frontendInvokeCommandNames.length}, definitions=${rustCommandDefinitionNames.length}, registered=${registeredTauriCommandNames.length}, handler_blocks=${tauriHandlerBlocks.length}, invoke_owners=${frontendInvokeOwnershipViolations.join(",")}, frontend_without_definition=${frontendCommandsMissingDefinitions.join(",")}, definitions_without_frontend=${rustCommandsMissingFrontendInvokes.join(",")}, definitions_without_registration=${definedCommandsMissingRegistration.join(",")}, registrations_without_definition=${registeredCommandsMissingDefinitions.join(",")}`
);
assert(
  oversizedProductionRustModules.length === 0,
  `Desktop Rust production modules exceeded the 1,200-line cohesion budget: ${oversizedProductionRustModules
    .map(({ entry, lines }) => `${entry} (${lines})`)
    .join(", ")}`
);
assert(
  criticalDesktopAgentModules.length === criticalDesktopAgentModuleBudgets.size &&
    implicitCriticalDesktopAgentModules.length === 0 &&
    oversizedCriticalDesktopAgentModules.length === 0 &&
    criticalDesktopAgentModules.every(
      ({ entry, source }) =>
        ["permission_service.rs", "session_output_cache_store.rs"].includes(entry) ||
        source.includes("use crate::")
    ) &&
    rustCompositionRoot.includes("mod memory_runtime;") &&
    !read("apps/desktop/src-tauri/src/knowledge_runtime.rs").includes(
      "memory_runtime"
    ) &&
    read("apps/desktop/src-tauri/src/memory_runtime.rs").includes(
      "knowledge_runtime::"
    ) &&
    !read("apps/desktop/src-tauri/src/runtime_values.rs").includes("std::fs") &&
    !read("apps/desktop/src-tauri/src/runtime_values.rs").includes("SqliteStore") &&
    read("apps/desktop/src-tauri/src/event_persistence.rs").includes(
      "event_security::"
    ) &&
    !read("apps/desktop/src-tauri/src/tool_execution.rs").includes(
      "fn append_event("
    ),
  `Critical desktop agent modules must use explicit crate boundaries and bounded ownership: implicit=${implicitCriticalDesktopAgentModules
    .map(({ entry }) => entry)
    .join(",")}, oversized=${oversizedCriticalDesktopAgentModules
    .map(({ entry, lines, budget }) => `${entry} (${lines}/${budget})`)
    .join(",")}`
);
assert(
  desktopAdapterModules.length === desktopAdapterModuleBudgets.size &&
    oversizedDesktopAdapterModules.length === 0 &&
    desktopEventOwnershipViolations.length === 0 &&
    rustCompositionRoot.includes("mod desktop_event_sink;") &&
    desktopEventSinkSource.includes("trait DesktopEventSink") &&
    desktopEventSinkSource.includes(
      "impl DesktopEventSink for tauri::AppHandle"
    ) &&
    desktopEventSinkSource.includes('"model-stream-delta"') &&
    desktopEventSinkSource.includes('"session-title-updated"'),
  `Desktop event adapter boundary regressed: oversized=${oversizedDesktopAdapterModules
    .map(({ entry, lines, budget }) => `${entry} (${lines}/${budget})`)
    .join(",")}, event_owners=${desktopEventOwnershipViolations.join(",")}`
);
assert(
  oversizedAgentCoreModules.length === 0,
  `Agent-core production modules exceeded the 1,450-line cohesion budget: ${oversizedAgentCoreModules
    .map(({ file, lines }) => `${file} (${lines})`)
    .join(", ")}`
);
assert(
  modelProviderModuleBudgets.size === modelProviderModules.length &&
    modelProviderModules.every(({ entry }) => modelProviderModuleBudgets.has(entry)) &&
    oversizedModelProviderModules.length === 0,
  `Model provider streaming boundaries regressed: ${oversizedModelProviderModules
    .map(
      ({ entry, lines }) =>
        `${entry} (${lines}/${modelProviderModuleBudgets.get(entry)})`
    )
    .join(", ")}`
);
assert(
  toolsModuleBudgets.size === toolsModules.length &&
    toolsModules.every(({ entry }) => toolsModuleBudgets.has(entry)) &&
    oversizedToolsModules.length === 0 &&
    read("crates/tools/src/lib.rs").includes("mod file_tools;") &&
    read("crates/tools/src/lib.rs").includes("mod meta_tools;") &&
    read("crates/tools/src/lib.rs").includes("mod image_generation;") &&
    read("crates/tools/src/lib.rs").includes("mod web_search;") &&
    toolsSource.includes("web_search_is_network_permissioned_but_effect_read_only") &&
    toolsSource.includes(".with_effect_semantics(ToolEffectSemantics::ReadOnly)"),
  `Tool ownership boundaries regressed: ${oversizedToolsModules
    .map(({ entry, lines }) => `${entry} (${lines}/${toolsModuleBudgets.get(entry)})`)
    .join(", ")}`
);
assert(
  agentStorageSource.includes("idx_events_task_kind_sequence") &&
    agentRecoveryServiceSource.includes(".list_by_task_and_kinds(") &&
    agentRecoveryServiceSource.includes("agent_events_for_session(store, &task_id, session_id)") &&
    promptEvolutionWorkerSource.includes(
      '.list_by_task_and_metadata(\n            &crate::runtime_values::phase16_task_id(),\n            "background_evaluation",\n            "true",'
    ) &&
    promptPairwiseRuntimeSource.includes(
      '.list_by_task_and_metadata(task_id, "project_id", project_id)'
    ),
  "Startup recovery and prompt evolution must keep history reads indexed and scope-bounded"
);
assert(
  agentStorageSource.includes("pub fn compare_exchange_read_model(") &&
    promptEvolutionReadModelSource.includes("compare_exchange_read_model(") &&
    promptEvolutionReadModelSource.includes(
      "prompt evolution snapshot publication conflicted twice"
    ) &&
    promptEvolutionRuntimeSource.includes("append_prompt_rollout_update(") &&
    !promptEvolutionRuntimeSource.includes("save_prompt_evolution_read_model") &&
    promptLearningOutboxProjectionSource.includes("list_by_task_after(") &&
    promptLearningOutboxProjectionSource.includes("compare_exchange_read_model(") &&
    orchestratorSource.includes("pub struct PromptLearningOutboxProjection") &&
    orchestratorSource.includes("pub struct PromptAutoTransferIntent") &&
    orchestratorSource.includes("pub struct PromptProDistillationIntent") &&
    orchestratorSource.includes("insert_auto_transfer_intent") &&
    orchestratorSource.includes("insert_pro_distillation_intent") &&
    orchestratorSource.includes("pending_payloads_are_valid") &&
    orchestratorSource.includes("PROMPT_LEARNING_OUTBOX_MAX_PENDING") &&
    orchestratorSource.includes("left.sequence") &&
    !promptLearningOutboxProjectionSource.includes("struct PendingIntentEnvelope") &&
    !rustLib.includes(".insert_auto_transfer(") &&
    !rustLib.includes(".insert_pro_distillation(") &&
    rustLib.includes(
      "matches_dispatch_marker(project_id, intent_id, &event.metadata)"
    ) &&
    !rustLib.includes("struct PromptAutoTransferIntent") &&
    !rustLib.includes("struct PromptProDistillationIntent"),
  "Prompt learning control must keep typed identity and FIFO state in orchestrator while desktop retains canonical-event, CAS, and delta adapters"
);
assert(
  shippingOrchestratorExamples.length === 0 &&
    fs.existsSync(
      path.join(root, "crates", "orchestrator-eval", "examples", "arena_lab.rs")
    ) &&
    fs.existsSync(
      path.join(
        root,
        "crates",
        "orchestrator-eval",
        "examples",
        "evaluation_v2_lab.rs"
      )
    ) &&
    !orchestratorSource.includes("mod arena;") &&
    !orchestratorSource.includes("mod benchmark;") &&
    !orchestratorSource.includes("mod fugu_evaluation;"),
  "Shipping orchestrator must not compile research evaluation modules or examples"
);
const workflowTopologyLearningSource = read(
  "crates/orchestrator/src/routing/workflow_topology_learning.rs"
);
assert(
  orchestratorSource.includes("mod workflow_topology_learning;") &&
    orchestratorSource.includes("pub use workflow_topology_learning::*;") &&
    workflowTopologyLearningSource.includes("pub struct WorkflowSearchTeacher") &&
    workflowTopologyLearningSource.includes("pub fn pareto_front") &&
    !workflowTopologyLearningSource.includes("use super::*;"),
  "Offline topology learning must remain isolated from the online router with explicit dependencies"
);
assert(
  appLineCount <= 2_360 &&
    appUseStateCount <= 25 &&
    appUseStateCounterProbe === 3 &&
    settingsPageLineCount <= 1_400 &&
    sessionThreadLineCount <= 1_150 &&
    inspectorLineCount <= 1_350 &&
    oversizedExtractedDesktopBoundaries.length === 0 &&
    tauriBridgeImplementationLineCount <= 2_620 &&
    tauriTypesLineCount <= 800 &&
    tauriNativeTypesLineCount <= 80 &&
    oversizedDesktopControllers.length === 0 &&
    desktopControllerEntries.every((entry) =>
      appSource.includes(`./controllers/${entry.replace(/\.ts$/, "")}`)
    ) &&
    appSource.includes('./appShellModel') &&
    appSource.includes('./controllers/useAppWorkspaceProjection') &&
    appSource.includes('./controllers/useAppShellController') &&
    appSource.includes('./controllers/useComposerAttachments') &&
    appSource.includes('./controllers/useComposerDrafts') &&
    appSource.includes('./controllers/useLatestAsyncSelection') &&
    appSource.includes('./controllers/usePermissionReviewController') &&
    appSource.includes('./controllers/useSidebarResize') &&
    appSource.includes('./components/WorkspaceChrome') &&
    settingsPageSource.includes('./SettingsModelsPanel') &&
    settingsPageSource.includes('./SettingsPermissionsPanel') &&
    settingsPageSource.includes('./SettingsToolsPanel') &&
    settingsModelsPanelSource.includes('./PromptEvolutionPanel') &&
    sessionThreadSource.includes('./AgentMarkdown') &&
    sessionThreadSource.includes('./SessionThreadArtifacts') &&
    sessionThreadSource.includes('./SessionThreadNavigation') &&
    sessionThreadSource.includes('./SessionToolChain') &&
    sessionThreadSource.includes('./SessionMinimap') &&
    sessionThreadSource.includes('./useSessionMinimapInteraction') &&
    tauriBridgeImplementation.includes('export type * from "./tauriTypes";') &&
    appShellStateModelSource.includes("transitionAppShellView") &&
    appShellControllerSource.includes("useReducer(") &&
    !appShellControllerSource.includes("useEffect") &&
    !appShellControllerSource.includes("invoke(") &&
    !appSource.includes("setWorkspaceViewBeforeSettings") &&
    !appSource.includes("setInspectorOpenBeforeSettings") &&
    !appSource.includes("setInspectorOpenBeforeSchedule") &&
    unguardedTauriFallbacks.length === 0 &&
    appSource.includes('import("./components/SettingsPage")') &&
    appSource.includes("<SettingsPage") &&
    appSource.includes("<Suspense") &&
    !appSource.includes('className="settings-sidebar"'),
  `Desktop boundaries regressed (App=${appLineCount}, useState=${appUseStateCount}, Settings=${settingsPageLineCount}, SessionThread=${sessionThreadLineCount}, Inspector=${inspectorLineCount}, extracted=${oversizedExtractedDesktopBoundaries
    .map(([entry, source, budget]) => `${entry}:${source.split("\n").length}/${budget}`)
    .join(",")}, Tauri=${tauriBridgeImplementationLineCount}, Types=${tauriTypesLineCount}, NativeTypes=${tauriNativeTypesLineCount}, controllers=${oversizedDesktopControllers
    .map(({ entry, lines }) => `${entry}:${lines}`)
    .join(",")}, unguarded catches=${unguardedTauriFallbacks.join(",")})`
);
assert(
  styleModuleEntries.every((entry) =>
    styleEntry.includes(`@import "./styles/${entry}";`)
  ) &&
    (styleEntry.match(/@import /g)?.length ?? 0) === styleModuleEntries.length &&
    !styleEntry.includes("{") &&
    oversizedStyleModules.length === 0,
  `Desktop styles must retain ordered domain modules below 1,900 lines: ${oversizedStyleModules
    .map(({ entry, lines }) => `${entry} (${lines})`)
    .join(", ")}`
);
for (const requiredModule of [
  "attachment_commands.rs",
  "attachment_upload_batches.rs",
  "agent_conductor_runtime.rs",
  "agent_loop_runtime.rs",
  "agent_runtime_snapshot.rs",
  "adaptive_collaboration_setup.rs",
  "adaptive_collaboration_execution.rs",
  "adaptive_collaboration_finalization.rs",
  "configuration_persistence.rs",
  "desktop_prelude.rs",
  "event_persistence.rs",
  "event_security.rs",
  "prompt_evaluation_runtime.rs",
  "prompt_evolution_hot_state.rs",
  "prompt_evolution_runtime.rs",
  "prompt_learning_outbox_projection.rs",
  "project_session_persistence.rs",
  "routing_learning_runtime.rs",
  "runtime_values.rs",
  "session_output_cache.rs",
  "sidecar_runtime.rs",
  "manual_tool_execution.rs",
  "tool_execution.rs",
]) {
  assert(
    desktopRustModules.some(({ entry }) => entry === requiredModule),
    `Desktop Rust architecture is missing ${requiredModule}`
  );
}

assert(packageJson.name === "cindx-desktop", "desktop package name changed");
assert(packageJson.scripts.dev.includes("vite"), "desktop dev script must run Vite");
assert(
  packageJson.scripts.test === "node --test tests/*.test.ts" &&
    frontendCheckScript.includes("npm test && npm run build") &&
    ciWorkflow.includes("run-quality-gates.mjs --profile ci-contract") &&
    releaseWorkflow.includes("npm --prefix apps/desktop test") &&
    qualityGateManifest.profiles["ci-contract"].includes("frontend-test") &&
    qualityGateManifest.profiles.full.includes("frontend-test"),
  "Frontend behavior tests must run locally, in CI, and before release"
);
assert(
  desktopDtoContract.schema === "cindx.desktop-dto-contract.v1" &&
    desktopDtoContract.cases.length >= 9 &&
    new Set(desktopDtoContract.cases.map((entry) => entry.name)).size ===
      desktopDtoContract.cases.length &&
    qualityGateById.get("desktop-dto-contract")?.command.join(" ") ===
      "node scripts/check-desktop-dto-contract.mjs" &&
    qualityGateById
      .get("desktop-dto-contract")
      ?.required_output.includes("cindx.desktop-dto-contract.v1") &&
    ["ci-contract", "control-plane", "full"].every((profile) =>
      qualityGateManifest.profiles[profile].includes("desktop-dto-contract")
    ) &&
    releaseWorkflow.includes("node scripts/check-desktop-dto-contract.mjs") &&
    desktopDtoContractRunner.includes("ContractMatches") &&
    desktopDtoContractRunner.includes("--typescript-only") &&
    desktopDtoContract.cases.some(
      (entry) => entry.name === "RuntimeStatus" && entry.tsModule === "tauriNativeTypes"
    ) &&
    desktopDtoContractRustTest.includes(
      "desktop_dto_contract_matches_committed_wire_values"
    ),
  "Critical Rust and TypeScript desktop DTOs must share a fail-closed release contract"
);
assert(packageJson.scripts.build.includes("vite build"), "desktop build script must build Vite");
assert(
  packageJson.scripts.tauri.includes("run-tauri.mjs") &&
    runTauriSource.includes('args[0] === "build"') &&
    runTauriSource.includes("nextPatchVersion") &&
    runTauriSource.includes("rollbackDesktopVersion") &&
    runTauriSource.includes("stable-aarch64-apple-darwin") &&
    runTauriSource.includes('["rev-parse", "HEAD"]') &&
    runTauriSource.includes("/^[0-9a-f]{40}$/") &&
    runTauriSource.includes('"status", "--porcelain=v1", "-z"') &&
    runTauriSource.includes('"diff", "--binary", "--no-ext-diff"') &&
    runTauriSource.includes('crypto.createHash("sha256")') &&
    runTauriSource.includes('"cindx.dirty-source.v1\\0"') &&
    runTauriSource.includes("CINDX_SOURCE_REVISION: sourceRevision"),
  "desktop Tauri runs must increment build versions, retain the Rust toolchain PATH, and stamp a verified source revision"
);
assert(
  releaseWorkflow.includes("tauriScript: ./node_modules/.bin/tauri"),
  "release workflow must bypass the local auto-versioning wrapper"
);
const githubSourceRevisionStamp = "CINDX_SOURCE_REVISION: ${{ github.sha }}";
const ciDirectTauriBuildSteps = ciWorkflow
  .split(/\n(?=      - name: )/)
  .filter((step) => step.includes("npm exec tauri build"));
const releaseDirectTauriBuildSteps = releaseWorkflow
  .split(/\n(?=      - name: )/)
  .filter((step) => step.includes("uses: tauri-apps/tauri-action@v1"));
assert(
  ciDirectTauriBuildSteps.length === 1 &&
    ciDirectTauriBuildSteps.every((step) => step.includes(githubSourceRevisionStamp)) &&
    releaseDirectTauriBuildSteps.length === 3 &&
    releaseDirectTauriBuildSteps.every((step) =>
      step.includes(githubSourceRevisionStamp)
    ),
  "every direct CI and release Tauri build must stamp the exact GitHub source revision"
);
assert(
  releaseWorkflow.includes("id: apple-signing") &&
    releaseWorkflow.includes("certificate_configured=false") &&
    releaseWorkflow.includes("notarization_configured=false") &&
    releaseWorkflow.includes(
      "APPLE_SIGNING_IDENTITY: ${{ steps.apple-signing.outputs.identity }}"
    ) &&
    releaseWorkflow.includes("Build and publish signed and notarized Universal app") &&
    releaseWorkflow.includes("Build and publish signed Universal app") &&
    unsignedReleaseStart >= 0 &&
    !unsignedReleaseBlock.includes("APPLE_") &&
    !releaseWorkflow.includes("APPLE_SIGNING_IDENTITY: ${{ env.APPLE_SIGNING_IDENTITY }}") &&
    !releaseWorkflow.includes("$GITHUB_ENV") &&
    !releaseWorkflow.includes("    env:\n      APPLE_CERTIFICATE:"),
  "release workflow must keep unsigned builds free of empty Apple signing variables"
);
assert(
  releaseWorkflow.includes("--target universal-apple-darwin --bundles app"),
  "release workflow must build a Universal app without the fragile DMG step"
);
assert(
  ciWorkflow.includes("retention-days: 7"),
  "main-branch test builds must have bounded artifact retention"
);
assert(
  ciWorkflow.includes("Stamp build version") &&
    ciWorkflow.includes('stamp-build-version.mjs "$GITHUB_RUN_NUMBER"') &&
    ciWorkflow.includes("Test build version carry rules") &&
    stampBuildVersionSource.includes("nextCindxVersion") &&
    versioningSource.includes("CINDX_MAX_MINOR = 10") &&
    versioningSource.includes("CINDX_MAX_PATCH = 100") &&
    stampBuildVersionSource.includes("apps/desktop/src-tauri/Cargo.lock"),
  "Every CI app build must stamp one synchronized monotonic version with carry rules"
);
assert(
  releaseVersionCheck.includes("does not match committed version"),
  "release workflow must reject mismatched tags"
);
assert(packageJson.dependencies.react, "React dependency is missing");
assert(packageJson.dependencies["@tauri-apps/api"], "Tauri API dependency is missing");
assert(
  packageJson.dependencies["playwright-core"] === "1.61.1",
  "Browser Control must pin playwright-core for reproducible sidecar behavior"
);
assert(packageJson.devDependencies["@tauri-apps/cli"], "Tauri CLI dependency is missing");

assert(tauriConfig.productName === "Cindx", "Tauri product name changed");
assert(
  /^\d+\.\d+\.\d+$/.test(packageJson.version) &&
    packageLock.version === packageJson.version &&
    packageLock.packages[""].version === packageJson.version &&
    tauriConfig.version === packageJson.version &&
    cargoToml.includes(`version = "${packageJson.version}"`) &&
    cargoLock.includes(`name = "cindx-desktop"\nversion = "${packageJson.version}"`),
  "Cindx version files must remain synchronized after every patch build"
);
assert(
  tauriConfig.build.devUrl === "http://127.0.0.1:5173",
  "Tauri devUrl should match Vite dev server"
);
assert(
  tauriConfig.build.frontendDist === "../dist",
  "Tauri frontendDist should point to Vite dist"
);
assert(
  tauriConfig.bundle.resources?.["../node_modules/playwright-core"] ===
    "sidecars/node_modules/playwright-core",
  "The app bundle must include playwright-core beside the browser sidecar"
);
assert(
  tauriConfig.app.security.csp?.["default-src"] === "'self'" &&
    tauriConfig.app.security.csp?.["connect-src"] === "ipc: http://ipc.localhost" &&
    tauriConfig.app.security.csp?.["img-src"]?.includes("data:") &&
    tauriConfig.app.security.csp?.["object-src"] === "'none'",
  "Production builds must enforce a restrictive Tauri CSP"
);
assert(
  tauriConfig.app.security.devCsp?.["connect-src"]?.includes("ws://127.0.0.1:5173") &&
    tauriConfig.app.security.devCsp?.["object-src"] === "'none'",
  "Development CSP must keep Tauri IPC and Vite HMR available"
);
assert(
  tauriConfig.app.windows.some((window) => window.title === "Cindx"),
  "Tauri window title is missing"
);
assert(
  tauriConfig.app.windows.every(
    (window) => window.titleBarStyle === "Overlay" && window.hiddenTitle === true
  ),
  "The macOS titlebar must hide its title and host the pane controls"
);
const titlebarHeight = 46;
const macOSTrafficLightButtonHeight = 14;
const macOSTrafficLightReplayInsetY =
  titlebarHeight - macOSTrafficLightButtonHeight;
assert(
  tauriConfig.app.windows.every((window) => window.visible === false) &&
    !rustLib.includes(".on_page_load(|webview, payload|") &&
    rustLib.includes("fn reveal_main_window(app: tauri::AppHandle)") &&
    rustLib.includes("schedule_main_window_reveal_fallback") &&
    rustLib.includes("MAIN_WINDOW_REVEAL_FALLBACK_MS: u64 = 12_000") &&
    rustLib.includes("fn repair_macos_traffic_light_position(") &&
    rustLib.includes("fn schedule_macos_traffic_light_position_repair(") &&
    rustLib.includes("MACOS_TRAFFIC_LIGHT_REPAIR_GENERATION") &&
    rustLib.includes(
      "MACOS_TRAFFIC_LIGHT_REPAIR_DELAYS_MS: [u64; 3] = [96, 320, 900]"
    ) &&
    rustLib.includes("for delay_ms in MACOS_TRAFFIC_LIGHT_REPAIR_DELAYS_MS") &&
    rustLib.includes("tauri::WindowEvent::Focused(_)") &&
    !rustLib.includes("tauri::WindowEvent::Focused(true)") &&
    tauriConfig.app.windows.every(
      (window) =>
        window.trafficLightPosition?.x === 14 &&
        window.trafficLightPosition?.y === macOSTrafficLightReplayInsetY &&
        macOSTrafficLightButtonHeight + window.trafficLightPosition.y ===
          titlebarHeight
    ) &&
    rustLib.includes("MACOS_TITLEBAR_HEIGHT: f64 = 46.0") &&
    rustLib.includes("fn centered_macos_traffic_light_origin_y(button_height: f64)") &&
    rustLib.includes(
      "origin.y = centered_macos_traffic_light_origin_y(button_frame.size.height);"
    ) &&
    rustLib.includes("repair_macos_traffic_light_position(&window)?;") &&
    rustLib.includes("let _ = repair_macos_traffic_light_position(&window);") &&
    rustLib.includes("tauri::WindowEvent::Resized(_)") &&
    rustLib.includes("tauri::WindowEvent::ScaleFactorChanged { .. }") &&
    !rustLib.includes("tauri::WindowEvent::Moved(_)") &&
    !rustLib.includes("tauri::WindowEvent::ThemeChanged(_)") &&
    tauriBridge.includes('invoke<void>("reveal_main_window")') &&
    appSource.includes("startupWindowRevealRequestedRef") &&
    appSource.includes("await Promise.race([") &&
    appSource.includes("document.fonts.ready") &&
    appSource.includes("window.setTimeout(resolve, 120)") &&
    appSource.includes("await revealMainWindow()") &&
    appSource.includes("!sessionRuntimeCache.hasAgent(state.activeSessionId)") &&
    !appSource.includes("agentState?.sessionId !== projectSessionState.activeSessionId") &&
    !appSource.includes("revealAfterStableFrame"),
  "The native window must keep framework and custom traffic-light geometry aligned across size, scale, focus, and background redraws"
);

assert(
  tauriConfig.app.windows.every(
    (window) =>
      window.trafficLightPosition?.y === macOSTrafficLightReplayInsetY
  ) &&
    styles.includes(`--titlebar-height: ${titlebarHeight}px`) &&
    styles.includes("--titlebar-control-size: 28px") &&
    styles.includes("grid-template-rows: var(--titlebar-height) minmax(0, 1fr)") &&
    styles.includes(
      "top: calc((var(--titlebar-height) - var(--titlebar-control-size)) / 2)"
    ) &&
    !styles.includes("--titlebar-content-offset-y"),
  "Native traffic lights must remain centered in the 46px app titlebar"
);
assert(
  tauriConfig.bundle.icon.includes("icons/icon.icns"),
  "Tauri bundle must use the generated macOS app icon"
);
assert(
  !localBuildScript.includes("local-build-number") &&
    localBuildScript.includes("nextCindxVersion(sourceVersion") &&
    localBuildScript.includes('"--version"') &&
    localBuildScript.includes("buildCompleted = true") &&
    localBuildScript.includes("if (!buildCompleted) restoreVersions()") &&
    localBuildScript.includes('args.has("--ephemeral-target")') &&
    localBuildScript.includes("CARGO_TARGET_DIR: targetRoot") &&
    localBuildScript.includes('path.join(os.homedir(), ".cargo", "bin")') &&
    localBuildScript.includes('CINDX_STARTUP_PROBE: "1"') &&
    localBuildScript.includes("Persistent-state failure probe did not fail closed") &&
    localBuildScript.includes('"--identifier"') &&
    localBuildScript.includes('const installApp = !args.has("--no-install")') &&
    localBuildScript.includes('run("pkill", ["-x", "cindx-desktop"]') &&
    localBuildScript.includes('waitForProcessExit("cindx-desktop")') &&
    packageJson.scripts?.["build:app"] === "node ../../scripts/build-local-app.mjs",
  "Local builds must persist successful versions, roll back failures, probe, sign, install, and package"
);
assert(
  tauriConfig.bundle.resources["../../../scripts/sidecars/browser-sidecar.js"] ===
    "sidecars/browser-sidecar.js" &&
    tauriConfig.bundle.resources["../../../scripts/sidecars/computer-sidecar.js"] ===
      "sidecars/computer-sidecar.js",
  "Packaged apps must carry browser and computer sidecar resources"
);
assert(
  capability.permissions.includes("core:default"),
  "Default Tauri capability should include core permissions"
);
assert(
  capability.permissions.includes("core:window:allow-start-dragging"),
  "Custom titlebar must be allowed to start native window dragging"
);

assert(html.includes("/src/main.tsx"), "index.html must load the React entrypoint");
assert(viteConfig.includes('base: "./"'), "Vite should emit relative asset paths for Tauri");
assert(mainSource.includes("<App />"), "React entrypoint must render App");
assert(appSource.includes("<SessionThread"), "App must render the session thread");
assert(
  rustLib.includes("run_started_at_ms") &&
    appSource.includes("runStartedAtMs={activeAgentState?.runStartedAtMs ?? 0}") &&
    sessionThreadSource.includes("activeRunProgress(timeline, runStartedAtMs)") &&
    !sessionThreadSource.includes("Elapsed since this request was sent") &&
    !sessionThreadSource.includes("formatRunElapsed") &&
    sessionThreadSource.includes("const RunProgressStatus = memo") &&
    !sessionThreadSource.includes("runBudgetMs > 0"),
  "Running status must use the current request without rendering elapsed time"
);
assert(
  sessionThreadSource.includes('aria-label="Session thread"'),
  "Session thread must expose its semantic region"
);
assert(
  packageJson.dependencies["markdown-to-jsx"] &&
    packageJson.dependencies.mermaid &&
    packageJson.dependencies["markmap-lib"] &&
    packageJson.dependencies["markmap-view"] &&
    sessionThreadSource.includes('from "markdown-to-jsx"') &&
    sessionThreadSource.includes("disableParsingRawHTML: true") &&
    sessionThreadSource.includes("content={item.message.content}") &&
    sessionThreadSource.includes("streamingContent={streamAnswer}") &&
    sessionThreadSource.includes("function MarkdownCodeBlock") &&
    sessionThreadSource.includes("markdownDiagramForCode") &&
    sessionThreadSource.includes("renderDiagrams: !streaming") &&
    mermaidDiagramSource.includes('import("mermaid")') &&
    mermaidDiagramSource.includes('securityLevel: "strict"') &&
    mermaidDiagramSource.includes("MAX_DIAGRAM_SOURCE_LENGTH") &&
    mermaidDiagramSource.includes("useResolvedTheme") &&
    mermaidDiagramSource.includes("themeVariables") &&
    markmapDiagramSource.includes('import("markmap-lib")') &&
    markmapDiagramSource.includes('import("markmap-view")') &&
    markmapDiagramSource.includes("MAX_MINDMAP_SOURCE_LENGTH") &&
    markmapDiagramSource.includes("transformer.md.set({ html: false })") &&
    markmapDiagramSource.includes('theme === "dark" ? "markmap-dark"') &&
    resolvedThemeSource.includes("MutationObserver") &&
    markdownDiagramModelSource.includes('normalizedLanguage !== "mindmap"') &&
    sessionThreadSource.includes("component: MarkdownCodeBlock") &&
    sessionThreadSource.includes("<DiagramFullscreen") &&
    sessionThreadSource.includes("<Maximize2") &&
    sessionThreadSource.includes('aria-label="Copy code"') &&
    diagramFullscreenSource.includes("downloadDiagramPng") &&
    diagramFullscreenSource.includes("replaceForeignObjectsWithSvgText") &&
    diagramFullscreenSource.includes("createPortal") &&
    diagramFullscreenSource.includes('onClick={(event) => event.stopPropagation()}') &&
    diagramFullscreenSource.includes('event.key === "Enter" || event.key === " "') &&
    diagramFullscreenSource.includes("<ZoomOut") &&
    diagramFullscreenSource.includes("<ZoomIn") &&
    diagramFullscreenSource.includes("<Download") &&
    diagramFullscreenSource.includes("diagramViewportCenter(viewport)") &&
    diagramFullscreenSource.includes("diagramViewportCanPan(viewport)") &&
    diagramFullscreenSource.includes("diagramPanActivationReached") &&
    diagramFullscreenSource.includes("onPointerDown={startPan}") &&
    diagramFullscreenSource.includes("onPointerLeave={leavePan}") &&
    diagramFullscreenSource.includes("onPointerMove={movePan}") &&
    diagramFullscreenSource.includes("onPointerCancel={finishPan}") &&
    diagramFullscreenSource.includes('aria-label={pannable ? "Scrollable Mermaid diagram"') &&
    diagramFullscreenSource.includes("tabIndex={pannable ? 0 : undefined}") &&
    sessionThreadSource.includes("const messageSelectable = !isUser && !isAssistant") &&
    sessionThreadSource.includes('role={messageSelectable ? "button" : undefined}') &&
    sessionThreadSource.includes(
      "onClick={messageSelectable ? () => onSelect(item) : undefined}"
    ) &&
    sessionThreadSource.includes('showClipboardToast("Copied to clipboard")') &&
    sessionThreadSource.includes("navigator.clipboard.writeText(content)") &&
    sessionThreadSource.includes("component: MarkdownTable") &&
    styles.includes(".thread-markdown pre code") &&
    styles.includes(".thread-code-block-header") &&
    styles.includes(".thread-diagram-fullscreen") &&
    styles.includes(".thread-diagram-zoom-controls") &&
    styles.includes('.thread-diagram-fullscreen-viewport[data-pannable="true"]') &&
    styles.includes('.thread-mermaid-diagram[data-theme="dark"] svg text') &&
    styles.includes('.thread-mermaid-diagram[data-theme="dark"] svg foreignObject *') &&
    mermaidDiagramSource.includes("applyDarkDiagramLabelContrast") &&
    mermaidDiagramSource.includes('luminance > 0.179 ? "#171717" : "#f3f3f3"') &&
    styles.includes(".clipboard-toast") &&
    styles.includes(".thread-markdown-table-shell") &&
    styles.includes("border-collapse: separate") &&
    !/\.thread-markdown table\s*\{[^}]*min-width:\s*max-content/s.test(styles) &&
    agentMarkdownSource.includes('role="region"') &&
    agentMarkdownSource.includes('aria-label="Scrollable table"') &&
    agentMarkdownSource.includes("tabIndex={0}"),
  "Assistant messages must render safe Markdown, lazy Mermaid and Markmap mind maps, copyable code, and clipboard feedback"
);
assert(
  sessionThreadSource.includes("useLayoutEffect") &&
    sessionThreadSource.includes("knownMessageIdsRef") &&
    sessionThreadSource.includes('addEventListener("selectstart"') &&
    sessionThreadSource.includes("thread-message-arriving") &&
    styles.includes("@keyframes thread-message-arrive") &&
    styles.includes("@keyframes thread-markdown-block-arrive") &&
    styles.includes(".thread-message-arriving .thread-markdown > *"),
  "New assistant responses must arrive progressively without replaying animation on history"
);
assert(
  modelProviderSource.includes("complete_streaming_cancellable") &&
    modelProviderSource.includes("static HTTP_CLIENT: OnceLock<Client>") &&
    modelProviderSource.includes("pool_idle_timeout") &&
    modelProviderSource.includes("consume_streaming_body") &&
    modelProviderSource.includes("tokio::time::timeout(HTTP_POLL_INTERVAL") &&
    modelProviderCargo.includes(
      'reqwest = { version = "0.12.28", default-features = false, features = ["charset", "http2", "multipart", "rustls-tls-native-roots", "stream", "system-proxy"] }'
    ) &&
    cargoToml.includes(
      'rustls = { version = "0.23.42", default-features = false, features = ["ring"] }'
    ) &&
    cargoToml.includes('[profile.test.package."*"]\ndebug = 0') &&
    !modelProviderSource.includes('Command::new("/usr/bin/curl")') &&
    modelProviderSource.includes("streamed_tool_calls") &&
    modelProviderSource.includes("MODEL_REQUEST_CANCELLED") &&
    rustLib.includes("agent_run_controls") &&
    rustLib.includes("agent_run_should_stop") &&
    runControlSource.includes("struct AgentRunControl") &&
    runControlSource.includes("DeadlineExceeded") &&
    runControlSource.includes("TurnBudgetExhausted") &&
    agentRuntimeSource.includes("AgentAdvance::TurnBudgetExhausted") &&
    rustLib.includes("request_agent_run_cancel") &&
    rustLib.includes("emit_agent_stream_delta") &&
    appSource.includes("<LiveSessionThread") &&
    !appSource.includes("subscribeToModelStream") &&
    sessionThreadFileSource.includes("useModelStreamAnswer") &&
    sessionThreadFileSource.includes("streamingContent={streamAnswer}") &&
    modelStreamAnswerSource.includes("subscribeToModelStream") &&
    modelStreamAnswerSource.includes("startModelStreamSubscription") &&
    modelStreamAnswerSource.includes('document.addEventListener("visibilitychange"') &&
    modelStreamSubscriptionSource.includes("router.route(payload, sessionId)") &&
    modelStreamSubscriptionSource.includes("accumulator.finish()") &&
    modelStreamEventRouterSource.includes('AGENT_STREAM_TASK_ID = "phase-16-agent-loop"') &&
    modelStreamEventRouterSource.includes("payload.sessionId === null") &&
    modelStreamEventRouterSource.includes("resetCandidateId") &&
    modelStreamEventRouterSource.includes("prepareForNextRequest") &&
    modelStreamEventRouterSource.includes("MODEL_STREAM_RETIRED_REQUEST_LIMIT") &&
    modelStreamAccumulatorSource.includes("adaptiveDelay") &&
    modelStreamAccumulatorSource.includes("URGENT_PENDING_LENGTH") &&
    modelStreamAccumulatorSource.includes("pendingSlabs") &&
    modelStreamAccumulatorSource.includes("hasUnpublishedSnapshot") &&
    streamingMarkdownModelSource.includes("settledChunks") &&
    streamingMarkdownModelSource.includes("tailId") &&
    streamingMarkdownModelSource.includes("scannedCharacters") &&
    streamingMarkdownModelSource.includes("STREAMING_MARKDOWN_SETTLED_CHUNK_LIMIT") &&
    streamingMarkdownTailBufferSource.includes("nextParseAt") &&
    streamingMarkdownTailBufferSource.includes("deferredTailRoot") &&
    streamingMarkdownTailBufferSource.includes("insertLeaf") &&
    agentMarkdownSource.includes("SettledMarkdownChunks") &&
    agentMarkdownSource.includes("StreamingMarkdownDeferredTail") &&
    streamingMarkdownDeferredTailSource.includes('data-fenced="true"') &&
    styles.includes(".thread-markdown-deferred-tail") &&
    !sessionThreadFileSource.includes("streamBufferRef") &&
    !agentMarkdownSource.includes("splitStreamingMarkdown") &&
    appSource.includes("markSessionBusy(sessionId, false)") &&
    tauriBridge.includes("sessionId: string | null") &&
    tauriBridge.includes("reset: boolean"),
  "Agent output must stream by session and Stop must cancel the active provider request"
);
assert(
  runControlSource.includes("from_snapshot_for_continuation") &&
    agentRuntimeSource.includes("user_cancelled_snapshot_cannot_continue") &&
    agentRuntimeSource.includes("permission_resume_preserves_consumed_budget") &&
    agentRuntimeSource.includes(
      "continuation_starts_a_fresh_bounded_segment_after_budget_exhaustion"
    ) &&
    rustLib.includes("begin_agent_run_control_for_continuation") &&
    rustLib.includes("suspended_agent_run_control_snapshot"),
  "Paused long-running work must continue in a fresh bounded segment without weakening permission or cancellation semantics"
);
assert(
  rustLib.includes("mod tool_runtime_service;") &&
    rustLib.includes("completed_tool_result(&store, &invocation, workspace_root)") &&
    agentToolRuntimeSource.includes('TOOL_RESULT_SCHEMA: &str = "cindx.tool-result.v1"') &&
    toolRuntimeServiceSource.includes("tool_input_fingerprint") &&
    toolRuntimeServiceSource.includes("idempotent_replay") &&
    toolRuntimeServiceSource.includes("retryable_failure_is_not_replayed") &&
    agentToolRuntimeSource.includes("apply_tool_spec_runtime_metadata") &&
    desktopAgentToolRuntimeSource.includes("apply_tool_spec_runtime_metadata") &&
    toolExecutionSource.includes("apply_tool_spec_runtime_metadata") &&
    manualToolExecutionSource.includes("apply_tool_spec_runtime_metadata") &&
    toolsSource.includes("fn effect_spec(&self, _invocation: &ToolInvocation)") &&
    toolsSource.includes("fn meta_invoke_preserves_target_effect_semantics()") &&
    toolRuntimeServiceSource.includes(
      "fn deferred_file_write_verification_reads_the_target_arguments()"
    ) &&
    toolRuntimeServiceSource.includes(
      'input.get("name").and_then(serde_json::Value::as_str) != Some("file.write")'
    ) &&
    agentStorageSource.includes("list_by_task_and_tool_call_id") &&
    agentStorageSource.includes("idx_events_task_tool_call_sequence") &&
    agentStorageSource.includes("list_by_task_and_effect_fingerprint") &&
    agentStorageSource.includes("idx_events_task_effect_fingerprint_sequence") &&
    agentStorageSource.includes("event_scope_columns_v3") &&
    agentStorageSource.includes("event_queue_scope_v1"),
  "Tool execution must persist metrics and replay only exact non-retryable completed calls through an indexed journal"
);
assert(
  rustLib.includes("mod manual_tool_execution;") &&
    appStateSource.includes("manual_tool_execution_gate: Mutex<()>") &&
    manualToolExecutionSource.includes("let _execution_gate = execution_gate") &&
    manualToolExecutionSource.includes("fn prepare_manual_tool_execution(") &&
    manualToolExecutionSource.includes("fn perform_manual_tool_execution(") &&
    manualToolExecutionSource.includes("fn commit_manual_tool_execution(") &&
    (toolCommandsSource.match(/execute_manual_tool_invocation\(/g)?.length ?? 0) === 2 &&
    (knowledgeCommandsSource.match(/execute_manual_tool_invocation\(/g)?.length ?? 0) === 2 &&
    !toolCommandsSource.includes("execute_tool_invocation(&mut store") &&
    !knowledgeCommandsSource.includes("execute_tool_invocation(&mut store"),
  "Manual Phase 5 and Phase 8 tools must execute outside the global store lock"
);
assert(
  rustLib.includes("fn prepare_run_knowledge_contexts(") &&
    rustLib.includes("let plan = plan_agent_run(") &&
    rustLib.includes("&plan.decision") &&
    rustLib.includes("let retrieve_workspace = decision.retrieval.enabled()") &&
    rustLib.includes("let workspace_handle = retrieve_workspace.then") &&
    rustLib.includes("if !effort.uses_conductor()") &&
    rustLib.includes("if !should_evaluate_strategy_profile(") &&
    rustLib.includes("fn fast_policy_never_enters_prompt_evolution_selection()") &&
    rustLib.includes('"dynamic_conductor_v2"') &&
    rustLib.includes('"dynamic_conductor_replanned"') &&
    rustLib.includes('"dynamic_conductor_degraded_workflow"') &&
    rustLib.includes('"dynamic_conductor_degraded_direct"') &&
    agentConductorRuntimeSource.includes("conductor_model_sequence") &&
    agentConductorRuntimeSource.includes("attempt_conductor_decision") &&
    agentConductorRuntimeSource.includes("CONDUCTOR_MAX_ATTEMPTS") &&
    rustLib.includes("index_graph_chunks_cancellable") &&
    rustLib.includes("upsert_all(extractions)") &&
    ragSource.includes("index_workspace_cancellable") &&
    ragSource.includes("RAG_INDEX_CANCELLED") &&
    modelProviderSource.includes("embed_cancellable") &&
    graphSource.includes("pub fn upsert_all"),
  "Fast must remain direct while Auto/Pro retrieval is conductor-planned and cancellable"
);
assert(
  appSource.includes("sessionSelectionRequestRef") &&
    latestAsyncSelectionSource.includes("const runningRef = useRef(false)") &&
    latestAsyncSelectionSource.includes(
      "const pendingRef = useRef<PendingSelection<Value> | null>(null)"
    ) &&
    latestAsyncSelectionSource.includes("while (pendingRef.current)") &&
    latestAsyncSelectionSource.includes("pending.operation = operation") &&
    appSource.includes("sessionRefreshRequestRef") &&
    appSource.includes("enqueueProjectSessionSelection") &&
    appSource.includes("if (sessionId === activeSessionIdRef.current) {") &&
    appSource.includes("getContextState(sessionId)") &&
    appSource.includes("if (workspaceChanged) refreshWorkspaceScopedState()") &&
    !sessionRefreshBlock.includes("setPhase7(await getPhase7State())") &&
    tauriBridge.includes('invoke<ContextState>("get_context_state", {') &&
    rustLib.includes("session_id: Option<String>") &&
    rustLib.includes("project_session_metadata_for_session(&state, session_id.as_deref())"),
  "Session switching must prioritize chat state and defer workspace-wide refreshes"
);
assert(
  sessionRuntimeModelSource.includes("SESSION_STATE_CACHE_LIMIT = 24") &&
    sessionRuntimeModelSource.includes("class SessionRuntimeCache") &&
    appWorkspaceProjectionSource.includes("SESSION_STATE_CACHE_LIMIT") &&
    appSource.includes("sessionRuntimeCache") &&
    !appSource.includes("agentStateCacheRef") &&
    !appSource.includes("agentTraceCacheRef") &&
    !appSource.includes("contextStateCacheRef") &&
    appSource.includes("requestSessionAgentState(sessionId)") &&
    appSource.includes("applySelectedSessionAgentState") &&
    appSource.includes("prefetchedAgentState") &&
    appSource.includes("void Promise.all(") &&
    appSource.includes("restoreCachedSessionState(sessionId)") &&
    appSource.includes("startTransition(() =>") &&
    !appSource.includes("onSessionPrefetch") &&
    !sidebarSource.includes("onSessionPrefetch"),
  "Session switching must restore a bounded cache without speculative full-session reads"
);
assert(
  rustLib.includes("async fn get_agent_state(") &&
    rustLib.includes("async fn get_agent_state_delta(") &&
    rustLib.includes("async fn get_agent_state_revision(") &&
    rustLib.includes("async fn get_agent_trace_state(") &&
    rustLib.includes("async fn get_context_state(") &&
    rustLib.includes("let store = open_app_read_store()?") &&
    rustLib.includes("load_agent_session_read_model") &&
    rustLib.includes("list_by_task_and_metadata_before") &&
    rustLib.includes("list_by_task_and_metadata_after") &&
    rustLib.includes('"agent_run_id"') &&
    rustLib.includes("get_agent_history_page") &&
    !rustLib.includes("cached_agent_events_for_session") &&
    rustLib.includes("agent state load failed to join") &&
    rustLib.includes("agent trace load failed to join") &&
    rustLib.includes("context state load failed to join"),
  "Session reads must stay off the command thread and query only the requested session"
);
assert(
  agentStorageSource.includes("idx_events_task_session_sequence") &&
    agentStorageSource.includes("event_scope_columns_v3") &&
    agentStorageSource.includes("event_queue_scope_v1") &&
    agentStorageSource.includes("list_by_task_and_metadata_after") &&
    agentStorageSource.includes("save_read_model") &&
    rustLib.includes("AGENT_SESSION_READ_MODEL_NAMESPACE") &&
    rustLib.includes("mod collaboration_service") &&
    rustLib.includes("mod permission_service") &&
    rustLib.includes("mod queue_service") &&
    !rustLib.includes("mod run_lifecycle") &&
    rustLib.includes("mod session_projection") &&
    queueServiceSource.includes("struct QueuedAgentMessagePayload") &&
    runLifecycleSource.includes("enum AgentRunStatus") &&
    sessionProjectionSource.includes("struct AgentSessionReadModel") &&
    sessionProjectionSource.includes("load_agent_session_read_model_with_stats") &&
    rustLib.includes("struct AgentStateDelta") &&
    tauriBridge.includes("export async function getAgentStateDelta") &&
    sessionRuntimeModelSource.includes("function mergeAgentStateDelta") &&
    appSource.includes("mergeAgentStateDelta") &&
    appSource.includes("getAgentStateDelta(sessionId"),
  "Active session polling must use indexed event deltas and a persistent read model"
);
assert(
  runExecutionSource.includes("pub trait AgentRunExecutor") &&
    runExecutionSource.includes("pub fn execute_agent_run") &&
    agentRunEngineSource.includes("execute_agent_run(&mut executor, prepared)") &&
    !agentRunEngineSource.includes("loop {\n            let agent_model") &&
    agentCompletionRuntimeSource.includes("delivery_request_id") &&
    agentCompletionRuntimeSource.includes("terminal_selection_override") &&
    agentCompletionRuntimeSource.includes("persist_selected_terminal_message"),
  "Production execution must keep one application run driver and one terminal delivery stream"
);
assert(
  agentStorageSource.includes("pragma journal_mode = WAL") &&
    agentStorageSource.includes("pragma synchronous = NORMAL") &&
    agentStorageSource.includes("pragma busy_timeout = 5000") &&
    agentStorageSource.includes("pragma query_only = ON") &&
    rustLib.includes("failed to record agent progress") &&
    rustLib.includes("context preparation failed") &&
    rustLib.includes("skill context preparation failed"),
  "Agent storage must tolerate concurrent readers and report the failing run stage"
);
assert(
  tauriBridge.match(/if \(isTauriRuntime\(\)\) throw error;/g)?.length >= 10 &&
    sessionRuntimeModelSource.includes("function mergeAgentStateSnapshot") &&
    appSource.includes("mergeAgentStateSnapshot") &&
    appSource.includes("mergeAgentStateSnapshot(current, failedState)") &&
    appSource.includes("mergeAgentStateSnapshot(current, nextAgentState)"),
  "Real Tauri agent failures must propagate without replacing loaded session history"
);
assert(
  orchestratorSource.includes("pub struct WorkflowExecutionCheckpoint") &&
    orchestratorSource.includes("pub enum WorkflowStepStatus") &&
    orchestratorSource.includes("pub fn runnable_step_indices") &&
    orchestratorSource.includes("pub fn continue_with_budget") &&
    rustLib.includes("load_workflow_checkpoint_for_run") &&
    rustLib.includes("append_workflow_checkpoint_event") &&
    rustLib.includes("Collaboration workflow step checkpointed") &&
    rustLib.includes("WORKFLOW_RESUMABLE_ERROR_PREFIX") &&
    rustLib.includes("workflow_checkpoint.completed_outputs()") &&
    rustLib.includes("workflow_checkpoint.assign_step_credits") &&
    rustLib.includes("timeline_workflow_progress") &&
    tauriBridge.includes("workflowProgress?:") &&
    sessionThreadSource.includes("latestWorkflow.totalSteps") &&
    sessionThreadSource.includes("Checkpoint saved"),
  "Adaptive workflows must persist node checkpoints and resume only incomplete branches with fresh budget"
);
assert(
  appSource.includes("sessionLoadingId") &&
    appSource.includes("loading={sessionLoadingId === activeSession?.id && !activeAgentState}") &&
    sessionThreadSource.includes(
      'loading ? "Loading conversation" : "Cindx is ready for you."'
    ) &&
    sessionThreadSource.includes('className="session-thread-empty-orb"') &&
    sessionThreadSource.includes('state="solving"') &&
    sessionThreadSource.includes("size={64}") &&
    styles.includes(".session-thread-empty-orb") &&
    styles.includes("flex: 0 0 64px") &&
    styles.includes("min-width: 64px") &&
    styles.includes("min-height: 64px"),
  "Cold loads must stay explicit and ready empty sessions must show the 64px solving orb"
);
assert(
  sessionThreadSource.includes("export const SessionThread = memo(function SessionThread") &&
    sessionThreadSource.includes("const ToolChainDisclosure = memo(function ToolChainDisclosure") &&
    sessionThreadSource.includes("{open && (") &&
    sessionThreadSource.includes("threadContentRef") &&
    sessionThreadSource.includes("resizeObserver.observe(threadContentRef.current)") &&
    sessionThreadFileSource.includes("SessionThreadViewCache") &&
    sessionThreadFileSource.includes("viewCache.rowHeight") &&
    sessionThreadFileSource.includes(".loadArtifacts(sessionId") &&
    sessionThreadFileSource.includes("measureElement: (element, entry, instance)") &&
    sessionThreadFileSource.includes('data-session-id={sessionId ?? ""}') &&
    sessionThreadViewCacheSource.includes("while (sessionRows.size > this.rowLimit)") &&
    sessionThreadViewCacheSource.includes("while (this.rowHeights.size > this.sessionLimit)") &&
    sessionThreadViewCacheSource.includes("artifactReloads") &&
    !sessionThreadFileSource.includes("sessionTransitionRef") &&
    !sessionThreadFileSource.includes('status === "running" ? 180 : 0') &&
    !sessionThreadSource.includes('querySelectorAll<HTMLElement>("[data-minimap-kind]")') &&
    styles.includes(".thread-content") &&
    styles.includes("content-visibility: auto") &&
    styles.includes("contain-intrinsic-size: auto 96px") &&
    styles.includes(".thread-virtual-row > .thread-message") &&
    styles.includes("content-visibility: visible") &&
    styles.includes("contain-intrinsic-size: none"),
  "Long conversations must avoid hidden activity trees, row-by-row observation, and offscreen layout work"
);
assert(
  sessionThreadSource.includes("LATEST_OUTPUT_THRESHOLD") &&
    sessionThreadSource.includes("followLatestRef") &&
    sessionThreadSource.includes("historyScrollIntentRef") &&
    sessionThreadSource.includes("lastScrollTopRef") &&
    sessionThreadSource.includes("const pinLatestOutput = useCallback") &&
    sessionThreadSource.includes("const pauseLatestFollow = useCallback") &&
    !sessionThreadSource.includes("const movedTowardHistory =") &&
    sessionThreadSource.includes("const handleWheel = (event: WheelEvent)") &&
    sessionThreadSource.includes('thread.addEventListener("wheel", handleWheel') &&
    sessionThreadSource.includes("shortBounceAnimationRef.current = content.animate") &&
    sessionThreadSource.includes('window.matchMedia("(prefers-reduced-motion: reduce)")') &&
    sessionThreadSource.includes("const requestOlderHistoryIfNeeded = () =>") &&
    (sessionThreadSource.match(/requestOlderHistoryIfNeeded\(\);/g)?.length ?? 0) >= 3 &&
    sessionThreadSource.includes(
      "if (followLatestRef.current && viewportResized) pinLatestOutput()"
    ) &&
    sessionThreadSource.includes('className="thread-jump-latest"') &&
    sessionThreadSource.includes('aria-label="Jump to latest output"') &&
    styles.includes("backdrop-filter: saturate(150%) blur(18px)") &&
    styles.includes("@keyframes thread-jump-latest-in"),
  "Streaming output must stay pinned until user scroll intent, while short initial pages bootstrap older history"
);
assert(
  sessionThreadSource.includes("openExternalUrl") &&
    sessionThreadSource.includes("openArtifact") &&
    sessionThreadSource.includes("artifactLinkTarget") &&
    sessionThreadSource.includes("event.preventDefault()") &&
    appSource.includes("onLinkOpenError={setComposerError}") &&
    tauriBridge.includes('invoke<void>("open_external_url"') &&
    rustLib.includes("fn open_external_url") &&
    rustLib.includes('normalized.starts_with("https://")'),
  "Chat file and web links must open through validated native default-app commands"
);
assert(!appSource.includes("Agent Output"), "Primary model output must not be duplicated in the inspector");
assert(
  inspectorSource.includes('"details", "artifacts", "context"'),
  "Inspector must expose Details, Artifacts, and Context"
);
assert(
  inspectorSource.includes('className="inspector-debug-session"') &&
    inspectorSource.includes('aria-label="Copy session ID"') &&
    inspectorSource.includes("navigator.clipboard.writeText(sessionId)") &&
    styles.includes(".inspector-debug-session"),
  "Debug must expose the active session ID with a clipboard action"
);
assert(
  inspectorSource.includes("const [metadataOpen, setMetadataOpen] = useState(false)") &&
    inspectorSource.includes('className="metadata-details-toggle"') &&
    inspectorSource.includes("aria-expanded={metadataOpen}") &&
    inspectorSource.includes("data-open={metadataOpen}") &&
    inspectorSource.includes("data-motion={metadataMotion}") &&
    inspectorSource.includes('className="metadata-disclosure-icon"') &&
    inspectorSource.includes("setMetadataOpen(false);") &&
    styles.includes("@keyframes metadata-disclosure-opening") &&
    styles.includes("@keyframes metadata-disclosure-closing") &&
    /\.metadata-disclosure-icon \{[\s\S]*?transform: rotate\(0deg\);/.test(styles) &&
    /\.metadata-details\[data-open="true"\] \.metadata-disclosure-icon \{[\s\S]*?transform: rotate\(90deg\);/.test(
      styles
    ) &&
    /<span>Metadata<\/span>\s*<span\s+className="metadata-disclosure-icon"[\s\S]*?<ChevronRight[\s\S]*?className="metadata-disclosure-chevron"/.test(
      inspectorSource
    ) &&
    styles.includes(".metadata-details-body"),
  "Inspector metadata must use a trailing animated chevron disclosure"
);
assert(inspectorSource.includes("inspector-resize-handle"), "Inspector must remain resizable");
assert(
  !inspectorSource.includes("<strong>Inspector</strong>") &&
    !styles.includes(".inspector-header"),
  "Inspector tabs must lead the panel without a redundant visible title"
);
assert(
  inspectorSource.includes('<ol className="inspector-trace-list">') &&
    inspectorSource.includes('className="trace-sequence-item"') &&
    inspectorSource.includes('className="trace-sequence-marker"') &&
    inspectorSource.includes("Step ${index + 1}") &&
    inspectorSource.includes("Turn ${step.turnIndex}") &&
    styles.includes(".trace-sequence-item:not(:last-child)::after") &&
    styles.includes(".trace-sequence-marker"),
  "Agent trace must communicate execution order with a numbered connected timeline"
);
assert(
  workspaceChromeSource.includes(
    'aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}'
  ) &&
    !inspectorSource.includes("Close inspector"),
  "Inspector must use one stable toggle instead of duplicate controls"
);
assert(
  appShellStateModelSource.includes('inspectorOpen: false') &&
    appShellStateModelSource.includes('inspectorOpenBeforeSettings: false') &&
    appSource.includes("onArtifactInspect={(path) =>") &&
    appSource.includes("showInspectorOutput({ sessionId, path, nonce: Date.now() })") &&
    appShellStateModelSource.includes('type: "show_output"') &&
    appShellStateModelSource.includes("inspectorOpen: true, inspectorOutputRequest"),
  "Inspector must start closed and open only when the user inspects an in-thread output"
);
assert(
  appShellModelSource.includes(
    'DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible"'
  ) &&
    appShellModelSource.includes("loadDebugAlwaysVisible") &&
    settingsPageSource.includes("Always show Debug") &&
    appSource.includes("showDebug={debugAlwaysVisible}") &&
    inspectorSource.includes('hidden={!showDebug}') &&
    inspectorSource.includes("showDebug: boolean"),
  "Debug entry must stay hidden by default and use the persisted Settings preference"
);
assert(
  workspaceChromeSource.includes("window-toolbar") &&
    workspaceChromeSource.includes("data-open={sidebarOpen}") &&
    workspaceChromeSource.includes(
      'aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}'
    ),
  "The titlebar must expose a stable sidebar toggle"
);
assert(
  workspaceChromeSource.includes('className="window-toolbar" data-tauri-drag-region') &&
    styles.includes(".window-workspace-header") &&
    styles.includes("pointer-events: none") &&
    !appSource.includes("getCurrentWindow().startDragging()"),
  "The custom titlebar must expose a native drag region without blocking pane controls"
);
assert(
  appShellStateModelSource.includes('if (state.activeView === "settings")') &&
    appShellStateModelSource.includes(
      "return showWorkspaceView(state, state.workspaceViewBeforeSettings);"
    ) &&
    appShellControllerSource.includes("handleWorkspaceViewChange"),
  "Settings must toggle back to the previous workspace view"
);
assert(
  /function handleSelectSession\(sessionId: string\) \{\s*const leavingTimeline = activeView === "timeline";\s*showTimelineView\(\);\s*if \(sessionId === activeSessionIdRef\.current\) \{/.test(
    appSource
  ) && appSource.includes("if (!leavingTimeline) void acknowledgeSessionResult(sessionId);"),
  "Selecting any sidebar session must leave Settings, including the active session"
);
assert(
  workspaceChromeSource.includes("PanelLeftClose") &&
    workspaceChromeSource.includes("PanelLeftOpen") &&
    workspaceChromeSource.includes("PanelRightClose") &&
    workspaceChromeSource.includes("PanelRightOpen") &&
    styles.includes("window-pane-toggle-icon"),
  "Pane controls must animate between explicit open and close icons"
);
assert(
  styles.includes("transition: grid-template-columns 180ms") &&
    styles.includes("prefers-reduced-motion: reduce"),
  "Pane transitions must be subtle and respect reduced-motion preferences"
);
assert(
  appSource.includes("const SIDEBAR_MATERIAL_HIDE_DELAY_MS = 220") &&
    /\.app-shell\[data-sidebar-open="false"\] \.sidebar \{[^}]*opacity: 1;[^}]*visibility: hidden;[^}]*visibility 0s 180ms;/.test(styles) &&
    /\.inspector\[data-open="false"\] \{[^}]*opacity: 1;[^}]*visibility: hidden;[^}]*visibility 0s 180ms;/.test(styles) &&
    /\.app-shell\[data-sidebar-open="false"\] \.window-toolbar-panel-left,\s*\.app-shell\[data-inspector-open="false"\] \.window-toolbar-panel-right \{[^}]*width: 0;/.test(styles) &&
    styles.includes('.app-shell[data-sidebar-resizing="true"] .window-toolbar-panel-left') &&
    inspectorSource.includes("const INSPECTOR_MAX_WIDTH = 420") &&
    /:root\[data-theme="dark"\] \.inspector-debug \{[^}]*background: var\(--panel\);/.test(styles),
  "Pane close animations must keep opaque coverage, bound the inspector, and seal the dark debug edge"
);
assert(
  styles.includes("--radius-xs: 4px") &&
    styles.includes("--radius-sm: 6px") &&
    styles.includes("--radius-md: 8px") &&
    styles.includes("--radius-lg: 12px") &&
    styles.includes("--radius-xl: 20px") &&
    styles.includes("--radius-pill: 999px") &&
    styles.includes("--icon-radius: var(--radius-md)") &&
    styles.includes("window-toolbar::before"),
  "Icon feedback and full-height pane dividers must retain their polished geometry"
);
assert(
  workspaceChromeSource.includes("window-toolbar-panel-left") &&
    workspaceChromeSource.includes("window-toolbar-panel-right") &&
    appSource.includes("data-active-view={activeView}") &&
    appSource.includes("data-view={activeView}") &&
    styles.includes("--toolbar-solid: rgba(255, 255, 255, 0.96)") &&
    styles.includes("background: var(--toolbar-solid)") &&
    styles.includes('.workspace[data-view="timeline"]') &&
    styles.includes("--toolbar-glass: rgba(255, 255, 255, 0.76)") &&
    styles.includes("backdrop-filter: saturate(145%) blur(18px)") &&
    /\.window-toolbar-panel \{[\s\S]*?background: var\(--panel\);/.test(styles) &&
    tauriConfig.app.macOSPrivateApi === true &&
    tauriConfig.app.windows.every((window) => window.transparent === true) &&
    cargoToml.includes('features = ["macos-private-api"]'),
  "Timeline content must scroll beneath the translucent titlebar glass surface"
);
assert(
  /\.app-shell\[data-active-view="settings"\] \{[\s\S]*?--sidebar-layout-width: 0px;[\s\S]*?grid-template-columns: 0 minmax\(0, 1fr\) 0;[\s\S]*?transition: grid-template-columns 220ms var\(--ease-out-quart\);/.test(styles) &&
    appSource.includes('<div className="settings-transition-backdrop" aria-hidden="true" />') &&
    /\.settings-transition-backdrop \{[\s\S]*?position: absolute;[\s\S]*?z-index: 25;[\s\S]*?inset: 0;[\s\S]*?pointer-events: none;[\s\S]*?background: var\(--bg\);[\s\S]*?opacity: 0;[\s\S]*?transition: opacity 220ms var\(--ease-out-quart\);/.test(styles) &&
    /\.app-shell\[data-active-view="settings"\] \.settings-transition-backdrop \{[\s\S]*?opacity: 1;[\s\S]*?transition: none;/.test(styles) &&
    /\.workspace\[data-view="settings"\] \{[\s\S]*?z-index: 30;/.test(styles) &&
    /@media \(max-width: 1180px\)[\s\S]*?\.inspector\[data-open="true"\] \{[\s\S]*?z-index: 20;/.test(styles) &&
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.settings-transition-backdrop \{[\s\S]*?transition: opacity 0s 34ms;/.test(styles) &&
    /\.app-shell\[data-active-view="settings"\] \.window-toolbar,\s*\.app-shell\[data-active-view="schedule"\] \.window-toolbar \{[\s\S]*?background: transparent;/.test(styles) &&
    /\.app-shell\[data-active-view="settings"\] \.window-workspace-header,\s*\.app-shell\[data-active-view="schedule"\] \.window-workspace-header \{[\s\S]*?background: var\(--bg\);/.test(styles) &&
    /\.settings-view \{[\s\S]*?overflow-y: auto;[\s\S]*?scrollbar-gutter: stable;/.test(
      styles
    ) &&
    styles.includes("--scrollbar-size: 6px") &&
    styles.includes("*::-webkit-scrollbar-thumb") &&
    appSource.includes('activeView === "settings"') &&
    appSource.includes('? "Settings"') &&
    appSource.includes('{activeView !== "settings" && (') &&
    appSource.includes('open={activeView === "timeline" && inspectorOpen}'),
  "Settings must use a full-page stage while preserving the shared compact scrollbar"
);
assert(composerSource.includes('event.key !== "Enter"'), "Composer must support Enter to send");
assert(composerSource.includes("event.shiftKey"), "Composer must reserve Shift+Enter for a new line");
assert(
  composerSource.includes("COMPOSER_TEXTAREA_MIN_HEIGHT = 58") &&
    composerSource.includes("COMPOSER_TEXTAREA_MAX_HEIGHT = 180") &&
    composerSource.includes("textarea.scrollHeight") &&
    composerSource.includes('textarea.style.overflowY =') &&
    composerSource.includes("useLayoutEffect(() => {") &&
    composerSource.includes("rows={1}") &&
    /\.composer textarea \{[\s\S]*?min-height: 58px;[\s\S]*?max-height: 180px;[\s\S]*?overflow-y: hidden;/.test(
      styles
    ),
  "Composer must grow with its content before paint and stop at a bounded height"
);
assert(
  !composerSource.includes('if (working || canStop) return;') &&
    composerSource.includes("const showStop = agentActive && !hasInput") &&
    composerSource.includes('type={showStop ? "button" : "submit"}') &&
    composerSource.includes('data-mode={showStop ? "stop" : "send"}') &&
    composerSource.includes('onClick={showStop ? onCancel : undefined}') &&
    !composerSource.includes('className="composer-stop-button"') &&
    !composerSource.includes('disabled={working || canStop}\n              aria-keyshortcuts="Enter"'),
  "Composer must remain editable and use one adaptive send-or-stop primary action"
);
assert(
  composerSource.includes("onCompositionStart") &&
    composerSource.includes("onCompositionEnd") &&
    composerSource.includes("IME_POST_COMPOSITION_ENTER_GUARD_MS = 120") &&
    composerSource.includes("imeEnterSeenDuringCompositionRef") &&
    composerSource.includes("suppressImeEnterUntilRef") &&
    composerSource.includes("nativeEvent.keyCode === 229") &&
    composerSource.includes("textareaRef.current?.value ?? value") &&
    !composerSource.includes("compositionJustEndedRef") &&
    !composerSource.includes("window.setTimeout"),
  "Composer must not submit macOS IME candidate-selection keystrokes"
);
assert(
  /\.composer-stack > \.composer \{[^}]*width: min\(100%, 796px\);[^}]*margin-inline: auto;/.test(
    styles
  ),
  "Composer must stay centered and bounded when either workspace pane is collapsed"
);
assert(
  composerSource.includes("onPaste={(event) =>") &&
    composerSource.includes("event.clipboardData.items") &&
    composerSource.includes('item.type.startsWith("image/")') &&
    composerSource.includes("onPickAttachments(pastedImages)") &&
    composerSource.includes("function ComposerAttachmentPreview") &&
    composerSource.includes("useArtifactImagePreview(") &&
    composerSource.includes('className="composer-attachment-preview"') &&
    styles.includes('.composer-attachment[data-image="true"]') &&
    styles.includes(".composer-attachment-preview"),
  "Composer must stage and preview images pasted from the clipboard"
);
const removeAttachmentButton = composerSource.slice(
  composerSource.indexOf('aria-label={`Remove ${attachment.name}`}'),
  composerSource.indexOf('aria-label={`Remove ${attachment.name}`}') + 320
);
assert(
  removeAttachmentButton.includes("onRemoveAttachment(attachment)") &&
    !removeAttachmentButton.includes("disabled="),
  "Draft attachments must remain removable while the agent is running"
);
assert(
  composerSource.includes("<ChevronUp aria-hidden=\"true\" />") &&
    styles.includes('.composer-effort-control[data-open="true"] .composer-effort-trigger > svg') &&
    styles.includes("transform: rotate(180deg)"),
  "The upward-opening effort menu must point up when closed and down when open"
);
assert(
  composerSource.includes("const restoreKeyboardFocus = event.detail === 0") &&
    composerSource.includes("focus({ preventScroll: true })") &&
    /\.composer-toolbar-actions \{[\s\S]*?display: grid;[\s\S]*?width: 192px;[\s\S]*?grid-template-columns: 104px 36px 36px;/.test(
      styles
    ) &&
    /\.composer-primary-button \{[\s\S]*?width: 36px;[\s\S]*?min-width: 36px;[\s\S]*?max-width: 36px;/.test(
      styles
    ),
  "Effort selection must preserve keyboard focus without shifting the fixed primary action"
);
const voiceButtonPosition = composerSource.indexOf("<VoiceInputButton");
assert(
  voiceButtonPosition > composerSource.indexOf('className="composer-effort-control"') &&
    voiceButtonPosition < composerSource.indexOf('className="send-button composer-primary-button"') &&
    settingsModelsImplementationSource.includes('label="Speech recognition"') &&
    settingsModelsImplementationSource.includes("value={providerDraft.voiceModel}") &&
    voiceInputButtonSource.includes("disabled={disabled}") &&
    voiceInputButtonSource.includes('data-status={status}') &&
    voiceInputButtonSource.includes('status === "finishing"') &&
    voiceInputButtonSource.includes('state="working"') &&
    voiceInputButtonSource.includes('theme="dark"') &&
    styles.includes(".composer-voice-working") &&
    voiceInputImplementationSource.includes("navigator.mediaDevices.getUserMedia") &&
    voiceInputImplementationSource.includes("Microphone capture did not start") &&
    !voiceInputImplementationSource.includes("getByteTimeDomainData") &&
    voiceWebRtcRuntimeSource.includes("track.enabled = false") &&
    voiceInputImplementationSource.includes("activeRunRef.current === run") &&
    voiceInputImplementationSource.includes("run.sessionId !== sessionId") &&
    voiceInputImplementationSource.includes("run.sessionId, transcript") &&
    voiceInputImplementationSource.includes('type: "input_audio_buffer.clear"') &&
    voiceInputImplementationSource.includes('type: "input_audio_buffer.commit"') &&
    !voiceInputImplementationSource.includes('type: "response.create"') &&
    voiceInputImplementationSource.includes("const VOICE_CONNECT_TIMEOUT_MS = 30_000") &&
    voiceInputImplementationSource.includes("VOICE_FINISH_TIMEOUT_MS") &&
    voiceInputImplementationSource.includes("VOICE_COMMIT_DRAIN_MS") &&
    voiceInputImplementationSource.includes("updateVoiceDisconnectGrace") &&
    voiceInputImplementationSource.includes("forceDispose(false)") &&
    voiceWebRtcRuntimeSource.includes("window.clearTimeout(run.commitDelay)") &&
    voiceWebRtcRuntimeSource.includes("window.clearTimeout(run.disconnectTimeout)") &&
    voiceInputImplementationSource.includes("Voice connection did not produce an SDP offer") &&
    voiceInputModelSource.includes("conversation.item.input_audio_transcription.completed") &&
    !voiceInputModelSource.includes("conversation.item.input_audio_transcription.delta") &&
    voiceInputModelSource.includes("state.seenItemIds.includes(event.itemId)") &&
    composerSource.includes("composingRef.current || voiceBusy") &&
    composerSource.includes('if (pendingApproval) setVoiceStatus("idle")') &&
    composerDraftsSource.includes("appendDraftForSession") &&
    tauriBridgeImplementation.includes('invoke<VoiceSessionAnswer>("negotiate_voice_session"') &&
    tauriBridgeImplementation.includes('invoke<{ transcript: string }>("transcribe_voice_audio"') &&
    voiceInputImplementationSource.includes("transcribeVoiceAudio(pcmBase64)") &&
    voiceInputImplementationSource.includes("const MAX_RECORDING_SECONDS = 30") &&
    voiceInputImplementationSource.includes("stream.getTracks().forEach((track) => {") &&
    voiceInputImplementationSource.includes("node?.disconnect()") &&
    voiceInputImplementationSource.includes("await audioContext.close().catch") &&
    appBootstrapSource.includes("transcribe_voice_audio,") &&
    voiceCommandsSource.includes("pub(crate) async fn transcribe_voice_audio(") &&
    voiceCommandsSource.includes("MAX_PCM_BASE64_BYTES") &&
    voiceCommandsSource.includes("api_key: config.api_key") &&
    dashScopeRealtimeGuardSource.includes("const MAX_PCM_BYTES") &&
    dashScopeRealtimeProviderSource.includes("header::AUTHORIZATION") &&
    dashScopeRealtimeProviderSource.includes('format!("Bearer {}", config.api_key.trim())') &&
    dashScopeRealtimeProviderSource.includes('"type": "input_audio_buffer.commit"') &&
    !dashScopeRealtimeProviderSource.includes('"type": "response.create"') &&
    dashScopeAsrTaskProviderSource.includes('"action": action') &&
    dashScopeAsrTaskProviderSource.includes('"streaming": "duplex"') &&
    dashScopeAsrTaskProviderSource.includes('"function": "recognition"') &&
    dashScopeAsrTaskProviderSource.includes('Message::Binary') &&
    dashScopeAsrTaskProviderSource.includes('"finish-task"') &&
    rustLib.includes("OpenAiCompatibleRealtimeProvider") &&
    rustLib.includes("config.api_key") &&
    modelProviderSource.includes('format!("{endpoint}/realtime/calls")') &&
    modelProviderSource.includes('"type": "realtime"') &&
    modelProviderSource.includes('"model": model') &&
    modelProviderSource.includes('"transcription": { "model": "gpt-4o-mini-transcribe" }') &&
    modelProviderSource.includes('"turn_detection": null') &&
    microphoneInfoPlist.includes("NSMicrophoneUsageDescription") &&
    /\.composer-voice-button:hover:not\(:disabled\)[\s\S]*?background: #2563eb;/.test(styles) &&
    /\.composer-voice-button:hover:not\(:disabled\),[\s\S]*?filter: brightness\(0\.9\);/.test(styles) &&
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.composer-voice-loading,[\s\S]*?animation: none;/.test(styles),
  "Voice input must stay provider-configured, backend-authenticated, session-safe, and visibly live"
);
assert(
  tauriBridge.includes("attachments?: AgentAttachment[]") &&
    appSource.includes("attachments\n    };") &&
    rustLib.includes('"attachment_mime_types".to_string()') &&
    rustLib.includes("attachments: attachment_views_from_event(event)") &&
    sessionThreadSource.includes("function UserMessageAttachments") &&
    sessionThreadSource.includes('className="thread-message-attachments"') &&
    sessionThreadSource.includes("useArtifactImagePreview(attachment.path)") &&
    styles.includes('.thread-message-attachment[data-image="true"]'),
  "Sent attachments must persist with user messages and render as image or file bubbles"
);
assert(
  sessionThreadSource.includes("thread.scrollTop = thread.scrollHeight") &&
    sessionThreadSource.includes("useLayoutEffect(() => {") &&
    sessionThreadProjectionSource.includes(
      'message.sequence ?? `${message.role}-${index}`'
    ) &&
    appSource.includes("optimisticUserMessagesRef") &&
    appSource.includes("optimisticUserMessageRevision") &&
    appSource.includes("setOptimisticUserMessageRevision") &&
    appWorkspaceProjectionSource.includes("messagesWithOptimisticUserMessage") &&
    appSource.includes("messages={visibleAgentMessages}"),
  "Session thread must show submitted user messages before paint and preserve them during polling"
);
assert(
  rustLib.includes("candidate.sessions[index].seen_event_sequence = latest_sequence;") &&
    rustLib.includes("if session.archived {\n            continue;\n        }") &&
    rustLib.includes("fn archived_session_restore_does_not_revive_seen_activity()"),
  "Archiving or restoring a session must clear stale lifecycle status durably"
);
assert(
  sessionThreadSource.includes('className="thread-minimap"') &&
    sessionThreadSource.includes('role="scrollbar"') &&
    sessionThreadSource.includes("onPointerMove={handleMinimapPointerMove}"),
  "Session thread must expose an interactive left-side minimap"
);
assert(
  sessionThreadSource.includes("minimapMarkerPosition") &&
    sessionThreadSource.includes("MINIMAP_MARKER_GAP = 12") &&
    sessionThreadSource.includes("Math.max(0, markerCount - 1) / 2") &&
    sessionThreadSource.includes("const groupStart = (bounds.height - groupHeight) / 2") &&
    sessionThreadSource.includes("minimapMarkerPosition(index, markers.length)") &&
    sessionThreadSource.includes("MIN_MINIMAP_MARKERS = 2") &&
    sessionThreadSource.includes("data-edge-fade") &&
    sessionThreadSource.includes("data-wave-distance") &&
    sessionThreadSource.includes("thread-minimap-preview") &&
    sessionThreadSource.includes("setPreviewIndex(hoveredIndex), 420)"),
  "Minimap markers must grow evenly from the vertical center with a five-tick hover wave and delayed preview"
);
assert(
  sessionThreadSource.includes("MAX_MINIMAP_MARKERS = 32") &&
    sessionThreadProjectionSource.includes('role === "user"') &&
    sessionThreadProjectionSource.includes('role === "assistant"') &&
    sessionThreadProjectionSource.includes(
      'preview.toLowerCase() !== "tool request"'
    ) &&
    sessionThreadSource.includes("rowIndexByItemId.get(marker.id)"),
  "Minimap must index only sparse, substantive user and model output anchors"
);
assert(
    packageJson.dependencies["@tanstack/react-virtual"] &&
    sessionThreadFileSource.includes('from "@tanstack/react-virtual"') &&
    sessionThreadFileSource.includes("measureElement as measureVirtualElement") &&
    sessionThreadFileSource.includes("useVirtualizer") &&
    sessionThreadSource.includes("const rowVirtualizer = useVirtualizer") &&
    sessionThreadSource.includes("const virtualRows = rowVirtualizer.getVirtualItems()") &&
    sessionThreadSource.includes("rowVirtualizer.measureElement(element)") &&
    sessionThreadSource.includes("useAnimationFrameWithResizeObserver: true") &&
    !sessionThreadSource.includes("const measureRenderedRows = useCallback") &&
    !sessionThreadSource.includes("element.getBoundingClientRect().height") &&
    sessionThreadSource.includes("ref={measureThreadRow}") &&
    sessionThreadSource.includes('className="thread-virtual-list"') &&
    sessionThreadSource.includes('className="thread-virtual-row"') &&
    styles.includes(".thread-virtual-list") &&
    styles.includes(".thread-virtual-row"),
  "Long sessions must virtualize variable-height rows instead of mounting the full transcript"
);
assert(
  sessionThreadSource.includes("SESSION_THREAD_PROJECTION_CACHE_LIMIT = 4") &&
    sessionThreadSource.includes(
      "useRef<Map<string, SessionThreadProjection>>(new Map())"
    ) &&
    sessionThreadSource.includes("readSessionState(projectionCacheRef.current, sessionId)") &&
    sessionThreadSource.includes("SESSION_THREAD_PROJECTION_CACHE_LIMIT\n      );"),
  "Session switching must reuse a small bounded projection cache without retaining every long transcript"
);
assert(
  styles.includes(".session-thread::-webkit-scrollbar") &&
    styles.includes("scrollbar-width: none") &&
    styles.includes(".thread-minimap-position") &&
    styles.includes('.thread-minimap-marker[data-wave-distance="0"]') &&
    /\.thread-minimap-marker \{[\s\S]*?width: 20px;[\s\S]*?height: 1\.5px;[\s\S]*?scaleX\(0\.34\)/.test(
      styles
    ) &&
    /\.thread-minimap-position \{[\s\S]*?width: 20px;[\s\S]*?height: 2px;[\s\S]*?scaleX\(0\.34\)/.test(
      styles
    ),
  "Session thread must replace its native scrollbar with the minimap"
);
assert(
  !sessionThreadSource.includes('data-content-ready={contentReady}') &&
    sessionThreadSource.includes('className="session-thread-empty-state"') &&
    sessionThreadSource.includes("rowVirtualizer.measureElement(element)") &&
    appWorkspaceProjectionSource.includes("const sessionPrefetchKey = useMemo") &&
    appSource.includes("requestSessionAgentState(sessionId).catch(() => null)") &&
    appSource.includes("await Promise.all([worker(), worker()])") &&
    appSource.includes("const cached = sessionRuntimeCache.read(sessionId)") &&
    appSource.includes("setAgentState(cached.agent)") &&
    sessionRuntimeModelSource.includes("private readonly agentRequests") &&
    sessionRuntimeModelSource.includes("const existing = this.agentRequests.get(sessionId)") &&
    !styles.includes('.session-thread[data-content-ready="false"]'),
  "Initial session hydration must restore cached rows immediately and prewarm uncached sessions with bounded concurrency"
);
assert(
  sessionThreadSource.includes("thread-message-actions") &&
    sessionThreadSource.includes("threadTimeFormatter") &&
    sessionThreadSource.includes("editableStoppedUserMessageId") &&
    sessionThreadSource.includes('status === "cancelled"') &&
    sessionThreadSource.includes('latestEvent?.label === "Agent task cancelled"') &&
    sessionThreadSource.includes('aria-label="Edit stopped message"') &&
    sessionThreadSource.includes("onEditMessage") &&
    styles.includes(".thread-message-actions time") &&
    styles.includes(".thread-message-user:hover .thread-message-actions time"),
  "User messages must expose hover time, copy, and stop-only edit actions"
);
assert(
  sessionThreadSource.includes('event.key.toLowerCase() === "f"') &&
    sessionThreadSource.includes("Find in current session") &&
    sessionThreadSource.includes("threadFindMatches") &&
    sessionThreadSource.includes("data-thread-search-id") &&
    appSource.includes("sessionId={activeSession?.id ?? null}") &&
    styles.includes(".thread-find") &&
    styles.includes(".thread-message.thread-search-current"),
  "Cmd+F must search and navigate message content only within the active session"
);
assert(
  !sessionThreadSource.includes("thread-message-agent-meta") &&
    sessionThreadSource.includes("!isUser && !isAssistant") &&
    sessionThreadSource.includes("thread-streaming-status") &&
    !sessionThreadSource.includes('<strong>Cindx</strong>'),
  "Assistant output must begin without a robot icon or Cindx label"
);
assert(
  sessionThreadProjectionSource.includes("function appendRows") &&
    sessionThreadProjectionSource.includes(
      '!content || content === "tool request"'
    ) &&
    !sessionThreadProjectionSource.includes("containsToolActivity") &&
    sessionThreadProjectionSource.includes(
      "while (end < items.length && isActivityCandidate(items[end]))"
    ) &&
    sessionThreadSource.includes("thread-tool-chain") &&
    sessionThreadSource.includes("Agent actions") &&
    sessionThreadSource.includes('className="thread-tool-chain-chevron"') &&
    !sessionThreadSource.includes("toolChainStatus(row.items)") &&
    sessionThreadSource.includes("<ToolChainItem") &&
    styles.includes(".thread-tool-chain-items") &&
    styles.includes(".thread-tool-chain[open] > summary .thread-tool-chain-chevron") &&
    /\.thread-tool-chain > summary \{[\s\S]*?padding-inline: 2px;[\s\S]*?\}/.test(
      styles
    ) &&
    /\.thread-tool-chain,\s*\.thread-tool-chain:hover,[\s\S]*?background: transparent;[\s\S]*?border: 0;[\s\S]*?box-shadow: none;/.test(
      styles
    ) &&
    /\.thread-tool-chain > summary:focus-visible \{[\s\S]*?outline: 1px solid var\(--border-strong\);[\s\S]*?box-shadow: none;[\s\S]*?\}/.test(
      styles
    ),
  "All contiguous agent reasoning, collaboration, and tool activity must default to one parent disclosure"
);
assert(
  disclosureTriangleSource.includes('viewBox="0 0 16 16"') &&
    disclosureTriangleSource.includes("c.52 0 1 .28 1.26.73") &&
    !disclosureTriangleSource.includes('import { Triangle }') &&
    (sessionThreadSource.match(/<DisclosureTriangle/g)?.length ?? 0) >= 2 &&
    !inspectorSource.includes("DisclosureTriangle") &&
    !settingsPageSource.includes("DisclosureTriangle") &&
    settingsPageSource.includes("function SettingsChevron") &&
    settingsPermissionsPanelSource.includes("function DisclosureChevron") &&
    (settingsPageSource.match(/<SettingsChevron/g)?.length ?? 0) +
      (settingsPageSource.match(/<DisclosureChevron/g)?.length ?? 0) ===
      9 &&
    settingsPageSource.includes('className={action ? "settings-action-chevron" : "settings-disclosure-chevron"}') &&
    (inspectorSource.match(/<ChevronRight/g)?.length ?? 0) >= 1 &&
    inspectorSource.includes('className="inspector-debug-chevron"') &&
    !styles.includes("advanced-settings summary::before") &&
    styles.includes("details[open] > summary .disclosure-triangle") &&
    styles.includes("details[open] > summary .settings-disclosure-chevron > svg") &&
    styles.includes(".settings-action-chevron") &&
    styles.includes("place-items: center") &&
    styles.includes("vertical-align: middle") &&
    styles.includes("transform-box: fill-box") &&
    styles.includes("transition: transform 180ms") &&
    styles.includes(".secondary-button:hover:not(:disabled) .settings-action-chevron > svg"),
  "Settings and Inspector must use trailing animated chevrons while thread disclosures retain the shared marker"
);
assert(
  sessionThreadSource.includes("thread-thinking") &&
    sessionThreadSource.includes("Thinking") &&
    !sessionThreadSource.includes("LoaderCircle") &&
    styles.includes("@keyframes thinking-sheen") &&
    /\.thread-thinking > span,[\s\S]*?\.thread-thinking > small \{[\s\S]*?min-height: 16px;[\s\S]*?align-items: center;[\s\S]*?line-height: 16px;/.test(
      styles
    ) &&
    styles.includes("prefers-reduced-motion: reduce"),
  "Agent thinking state must align uncropped progress text without a spinner"
);
assert(
  sessionThreadSource.includes("threadTimeFormatter") &&
    sessionThreadSource.includes("formatThreadTime") &&
    sessionThreadSource.includes("<time") &&
    !styles.includes(".thread-message-agent-meta"),
  "User messages must reveal a compact 24-hour timestamp without restoring agent metadata"
);
assert(
  composerSource.includes("pendingApproval") &&
    composerSource.includes("composer-permission") &&
    composerSource.includes('role="alertdialog"') &&
    composerSource.includes("key={pendingApproval.requestId}") &&
    composerSource.includes('aria-labelledby="composer-permission-title"') &&
    composerSource.includes('aria-describedby="composer-permission-description"') &&
    composerSource.includes("permissionFocusTarget(") &&
    composerSource.includes("restoreComposerAfterPermissionRef") &&
    composerSource.includes("restoreRequest.sessionId === sessionId") &&
    composerSource.includes('focusTarget === "request"') &&
    composerSource.includes('focusTarget === "composer"'),
  "Agent permissions must announce and focus each request before returning focus to the composer"
);
assert(
  composerSource.includes(
    'className="composer-error" role="alert" aria-live="assertive" aria-atomic="true"'
  ),
  "Composer errors must announce atomically without becoming a focus target"
);
assert(
  composerSource.includes("composer-stop-icon") &&
    composerSource.includes("canRetryError") &&
    composerSource.includes("RotateCcw") &&
    composerSource.includes("canContinueRun") &&
    composerSource.includes("Run paused at a safety checkpoint.") &&
    composerSource.includes('aria-label="Dismiss error"') &&
    composerSource.includes("onDismissError") &&
    composerSource.includes("Send") &&
    !composerSource.includes("retryMode") &&
    !composerSource.includes("agent-control-button"),
  "Composer must expose queue-safe send, stop, retry, continue, and dismiss actions"
);
assert(
  rustLib.includes("pause_agent_loop_for_control_stop") &&
    rustLib.includes("resume_suspended_agent_run") &&
    rustLib.includes("MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS") &&
    rustLib.includes("error.is_retryable()") &&
    modelProviderSource.includes("classify_provider_failure") &&
    rustLib.includes('"continuation_available".to_string()') &&
    agentRecoveryServiceSource.includes(
      '"stop_reason".to_string(), "app_restarted".to_string()'
    ) &&
    rustLib.includes("startup_recovery_preserves_unfinished_agent_runs_as_continuations") &&
    tauriBridge.includes("canContinue: boolean"),
  "Safety stops and app restarts must retain resumable state and expose a continuation action"
);
assert(
  composerSource.includes('className="composer-toolbar"') &&
    composerSource.includes('className="composer-toolbar-actions"') &&
    styles.includes("width: 36px;") &&
    styles.includes("height: 36px;") &&
    styles.includes("border-radius: var(--radius-round);") &&
    /\.composer-primary-button\[data-mode="stop"\]:hover:not\(:disabled\) \{[\s\S]*?background: #e05b5b;[\s\S]*?filter: none;/.test(styles) &&
    styles.includes('.composer-primary-button[data-mode="stop"]:hover .composer-working-ring'),
  "Composer controls must keep one fixed primary slot that switches between send and stop"
);
assert(
  queueServiceSource.includes("struct QueuedAgentMessageView") &&
    queueServiceSource.includes("struct QueuedAgentMessageReceipt") &&
    queueServiceSource.includes("struct QueuedAgentMessageActionReceipt") &&
    rustLib.includes("async fn queue_agent_message(") &&
    rustLib.includes("async fn edit_queued_agent_message(") &&
    rustLib.includes("async fn delete_queued_agent_message(") &&
    rustLib.includes("async fn steer_queued_agent_message(") &&
    !rustLib
      .slice(
        rustLib.indexOf("fn enqueue_agent_message_inner("),
        rustLib.indexOf("fn edit_queued_agent_message(")
      )
      .includes("agent_state_for_session") &&
    queueServiceSource.includes("fn pending_queued_agent_messages(") &&
    rustLib.includes("fn queued_agent_message_from_read_model(") &&
    !rustLib
      .slice(
        rustLib.indexOf("async fn edit_queued_agent_message("),
        rustLib.indexOf("async fn run_next_queued_agent_message(")
      )
      .includes("agent_state_for_session") &&
    rustLib.includes("fn append_agent_queue_event(") &&
    rustLib.includes("fn run_next_queued_agent_message_blocking(") &&
    rustLib.includes("queued_agent_messages_are_durable_ordered_and_session_scoped") &&
    rustLib.includes("queued_agent_message_start_and_restore_are_replay_safe") &&
    rustLib.includes("queued_agent_messages_update_the_incremental_session_read_model") &&
    rustLib.includes("queue_events_do_not_change_a_terminal_agent_status") &&
    rustLib.includes("queued_messages_preserve_a_permission_waiting_run") &&
    appSource.includes("steeredQueuedMessageIdsRef") &&
    sessionRuntimeModelSource.includes("function committedSteerReconciliation(") &&
    appSource.includes("const resolution = committedSteerReconciliation(state, queueId)") &&
    appSource.includes('if (resolution === "pending") return;') &&
    appSource.includes("receipt.steerCommitted") &&
    queueServiceSource.includes("steer_committed: bool") &&
    tauriBridge.includes("steerCommitted: boolean") &&
    rustLib.includes("fn commit_queued_agent_steer(") &&
    rustLib.includes("require_queued_agent_message(store, session_id, queue_id)?") &&
    rustLib.includes("queued_steer_commit_revalidates_the_current_queue_item") &&
    rustLib
      .slice(
        rustLib.indexOf("fn run_agent_task_blocking_inner("),
        rustLib.indexOf("fn cancel_agent_task(")
      )
      .includes("with_immediate_transaction(|store|") &&
    tauriBridge
      .slice(
        tauriBridge.indexOf("export async function steerQueuedAgentMessage("),
        tauriBridge.indexOf("export async function runNextQueuedAgentMessage(")
      )
      .includes("steerCommitted: false") &&
    tauriBridge.includes("export type QueuedAgentMessage") &&
    tauriBridge.includes("export type QueuedAgentMessageReceipt") &&
    tauriBridge.includes("export type QueuedAgentMessageActionReceipt") &&
    tauriBridge.includes("export async function queueAgentMessage(") &&
    tauriBridge.includes("queueId?: string") &&
    tauriBridge.includes("export async function runNextQueuedAgentMessage(") &&
    appSource.includes("async function drainQueuedMessages(sessionId: string)") &&
    appSource.includes("suppressQueueDrainSessionIdsRef") &&
    appSource.includes("<QueuedMessages") &&
    queuedMessagesSource.includes('role="menuitem"') &&
    queuedMessagesSource.includes("onSteer") &&
    queuedMessagesSource.includes("onEdit") &&
    queuedMessagesSource.includes("onDelete") &&
    styles.includes(".queued-message-stack") &&
    styles.includes("bottom: calc(100% - 2px)") &&
    styles.includes("border-radius: var(--radius-lg) var(--radius-lg) 0 0") &&
    styles.includes(".queued-message + .queued-message") &&
    appSource.includes("applyQueuedMessageReceiptForSession") &&
    appSource.includes("applyQueuedMessageActionReceiptForSession") &&
    appSource.includes("optimisticQueuedMessagesRef") &&
    appSource.includes("optimisticallyDeletedQueuedMessagesRef") &&
    styles.includes(".composer-stack > .composer") &&
    appSource.includes("data-has-queued={Boolean(activeAgentState?.queuedMessages.length)}") &&
    styles.includes('.composer-stack[data-has-queued="true"] > .composer'),
  "Session-scoped queued messages must persist, drain safely, and expose Steer, Edit, and Delete"
);
assert(sidebarSource.includes("session-branch"), "Tasks must be nested below the active project");
assert(
  sidebarSource.includes("collapsedProjectIds") &&
    sidebarSource.includes('className="project-main"') &&
    sidebarSource.includes('className="project-disclosure"') &&
    sidebarSource.includes("aria-expanded={expanded}") &&
    sidebarSource.includes("handleProjectDisclosure(project, expanded)") &&
    /<strong>\{project\.name\}<\/strong>[\s\S]*?<ChevronRight[\s\S]*?className="project-disclosure-chevron"/.test(
      sidebarSource
    ) &&
    styles.includes(
      '.project-disclosure[aria-expanded="true"] .project-disclosure-chevron'
    ) &&
    sidebarSource.includes('<div className="nav-heading">Tasks</div>') &&
    !sidebarSource.includes('<div className="nav-heading">Sessions</div>'),
  "Each project must expose a trailing animated chevron and label its child sessions as Tasks"
);
assert(
  styles.includes(".project-item") &&
    styles.includes(".session-item") &&
    /\.project-row \{[\s\S]*?border: 0;[\s\S]*?border-radius: var\(--radius-md\);/.test(styles) &&
    /\.session-branch \{[\s\S]*?border-left: 0;/.test(styles) &&
    /\.session-item \{[\s\S]*?min-height: 30px;[\s\S]*?border-radius: var\(--radius-md\);/.test(styles) &&
    styles.includes(".settings-section {") &&
    styles.includes(".archived-session-row + .archived-session-row"),
  "Project and session rows must stay compact, borderless, and hierarchy-line free"
);
assert(
  sidebarSource.includes("sidebar-footer") &&
    sidebarSource.includes("sidebar-settings") &&
    !sidebarSource.includes("sidebar-trace") &&
    !sidebarSource.includes('aria-label="Agent trace"') &&
    !sidebarSource.includes("LayoutDashboard") &&
    !styles.includes(".sidebar-actions"),
  "Settings must remain the only sidebar footer destination"
);
assert(
  !sidebarSource.includes("settingsButtonRef") &&
    !sidebarSource.includes("getAnimations()") &&
    !sidebarSource.includes('transform: "rotate(360deg)"') &&
    sidebarSource.includes('onClick={() => onViewChange("settings")}') &&
    !styles.includes(".sidebar-settings svg") &&
    styles.includes("transition: grid-template-columns 220ms var(--ease-out-quart)") &&
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.app-shell,/.test(styles),
  "Settings must open with a reduced-motion-safe full-page expansion and no gear spin"
);
assert(
  sidebarSource.includes('if (!state || (active && unseenResult)) return null;') &&
    sidebarSource.includes('unseenResult={session.unseenResult}') &&
    sidebarSource.includes('activity === "idle" ? null : activity') &&
    sidebarSource.includes('state === "working"') &&
    sidebarSource.includes('"Session needs attention"') &&
    sidebarSource.includes('<LoaderCircle aria-hidden="true" />') &&
    styles.includes(".session-status-working svg") &&
    styles.includes("animation: spin 900ms linear infinite") &&
    /\.session-status \{[\s\S]*?right: 3px;[\s\S]*?width: 26px;[\s\S]*?height: 28px;/.test(
      styles
    ) &&
    /\.session-row:hover \.session-status,[\s\S]*?\.session-row:focus-within \.session-status \{[\s\S]*?opacity: 0;/.test(
      styles
    ) &&
    styles.includes(".session-row:focus-within .session-more"),
  "Session state and action menu must share one fixed trailing slot and swap on hover"
);
assert(
  appWorkspaceProjectionSource.includes("matchingSessionExists") &&
    appWorkspaceProjectionSource.includes("if (normalizedSidebarQuery)") &&
    sidebarSource.includes("searchActive") &&
    sidebarSource.includes("projectSessions = sessions.filter") &&
    sidebarSource.includes("selectSessionResult") &&
    sidebarSource.includes('"No results"'),
  "Sidebar search must expose matching sessions across projects and navigate to a selected result"
);
assert(
  (settingsPageSource.match(/<span>Back to App<\/span>/g)?.length ?? 0) === 1 &&
    !appSource.includes('className="workspace-page-navigation"') &&
    settingsPageSource.includes('className="settings-sidebar"') &&
    settingsPageSource.includes('className="workspace-return-button settings-app-return"') &&
    styles.includes(".workspace-return-button"),
  "Settings must expose the only in-page return to the app"
);
assert(
  sidebarSource.includes('icons/icon.png') && sidebarSource.includes("appIconUrl"),
  "Sidebar brand must use the high-resolution packaged app icon"
);
assert(
  sidebarSource.includes('className="brand-identity"') &&
    sidebarSource.includes('className="brand-mark-shell"') &&
    sidebarSource.includes('className="brand-name-shimmer" aria-hidden="true"') &&
    !styles.includes(".brand-mark-shell::after") &&
    !styles.includes("brand-mark-shimmer") &&
    styles.includes(".brand-identity:hover .brand-name-shimmer") &&
    styles.includes("animation: brand-name-shimmer 720ms var(--ease-out-quart) 1 both") &&
    styles.includes("@keyframes brand-name-shimmer") &&
    styles.includes(".brand-name-shimmer {") &&
    styles.includes("background-clip: text") &&
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.brand-name-shimmer[\s\S]*?animation: none;/.test(
      styles
    ) &&
    !sidebarSource.includes('title={activeProject?.root}'),
  "Sidebar brand hover must sweep only the glyph-clipped name, hide workspace paths, and respect reduced motion"
);
assert(!sidebarSource.includes("Local agent"), "Sidebar brand must not show the old subtitle");
assert(
  !sidebarSource.includes("{project.status}") &&
    !sidebarSource.includes("{session.detail}") &&
    sidebarSource.includes("SessionStatusIndicator") &&
    sidebarSource.includes("LoaderCircle") &&
    styles.includes(".session-status-working") &&
    styles.includes(".session-status-complete") &&
    styles.includes(".session-status-attention") &&
    styles.includes(".session-status-complete > span") &&
    styles.includes(".session-status-unseen.session-status-complete > span") &&
    styles.includes(".session-status-unseen.session-status-attention > span") &&
    sidebarSource.includes('if (!state || (active && unseenResult)) return null;') &&
    sidebarSource.includes("session-status-unseen") &&
    sidebarSource.includes("active={session.active}") &&
    sidebarSource.includes("unseenResult={session.unseenResult}") &&
    sidebarSource.includes('className="session-name"') &&
    !sidebarSource.includes("<strong>{session.name}</strong>") &&
    styles.includes(".session-name") &&
    !appSource.includes("trackedSessionTaskIdsRef") &&
    rustLib.includes("project_session_lifecycle(SessionLifecycleInput {") &&
    rustLib.includes("session.activity = projection.activity.to_string();") &&
    rustLib.includes("session.attention_reason = projection.attention_reason.map(str::to_string);") &&
    rustLib.includes("session.unseen_result = projection.unseen_result;"),
  "Sidebar rows must show normal-weight titles and only icon-only background task state"
);
assert(
  sidebarSource.includes("onSessionRename") &&
    sidebarSource.includes('text: "Rename"') &&
    sidebarSource.includes('className="session-rename-form"') &&
    sidebarSource.includes('aria-label="Save session name"') &&
    sidebarSource.includes('aria-label="Cancel session rename"') &&
    !sidebarSource.includes("window.prompt") &&
    styles.includes(".session-rename-input") &&
    appSource.includes("handleRenameSession") &&
    tauriBridge.includes('invoke<ProjectSessionState>("rename_session"'),
  "Session menu must provide inline editing through the persisted Tauri command"
);
assert(
  sidebarSource.includes("onProjectRename") &&
    sidebarSource.includes('className="project-rename-form"') &&
    appSource.includes("handleRenameProject") &&
    tauriBridge.includes('invoke<ProjectSessionState>("rename_project"') &&
    rustLib.includes("fn rename_project("),
  "Project menu must provide inline editing through the persisted Tauri command"
);
assert(sidebarSource.includes("onSessionFork"), "Session menu must expose fork");
assert(sidebarSource.includes("onSessionArchive"), "Session menu must expose archive");
assert(
  sidebarSource.includes("onSessionDelete") &&
    sidebarSource.includes("onProjectDelete") &&
    sidebarSource.includes('text: "Delete Session"') &&
    sidebarSource.includes('text: "Delete Project"') &&
    sidebarSource.includes("confirmDeleteAction") &&
    sidebarSource.includes("await confirmDeleteAction(target.kind, target.name)") &&
    !sidebarSource.includes('role="alertdialog"') &&
    tauriBridge.includes('invoke<boolean>("confirm_delete_action"') &&
    rustLib.includes("async fn confirm_delete_action(") &&
    rustLib.includes("fn show_native_delete_confirmation(") &&
    appSource.includes("handleDeleteProject") &&
    tauriBridge.includes('invoke<ProjectSessionState>("delete_project"') &&
    rustLib.includes("fn delete_project(") &&
    rustLib.includes("remove_project_from_config"),
  "Project and session menus must use a persisted delete flow with confirmation"
);
assert(
  traceStatusIconSource.includes("CheckCircle2") &&
    traceStatusIconSource.includes("BadgeCheck") &&
    traceStatusIconSource.includes("OctagonX") &&
    traceStatusIconSource.includes("ShieldX") &&
    traceStatusIconSource.includes("Ban") &&
    traceStatusIconSource.includes("CirclePause") &&
    traceStatusIconSource.includes("LoaderCircle") &&
    traceStatusIconSource.includes("ShieldQuestion") &&
    inspectorSource.includes("ToolActivityIcon") &&
    inspectorSource.includes("BrainCircuit") &&
    inspectorSource.includes("DatabaseZap") &&
    inspectorSource.includes("MessageSquareText") &&
    inspectorSource.includes("Route") &&
    sessionToolChainSource.includes("ToolActivityIcon") &&
    inspectorSource.includes('export type InspectorTab = "trace"') &&
    inspectorSource.includes('(["trace", "details", "artifacts", "context"]') &&
    inspectorSource.includes("sessionTraceSteps.map((step, index)") &&
    inspectorSource.includes("debugBodyMounted") &&
    inspectorSource.includes("onTraceExport") &&
    styles.includes("grid-template-columns: repeat(4, minmax(0, 1fr));") &&
    inspectorSource.includes("<TraceStatusIcon status={agentStatus}") &&
    inspectorSource.includes("<TraceStatusIcon status={step.status}") &&
    inspectorSource.includes("<TraceStatusIcon status={traceStep.status}") &&
    !appSource.includes('className="trace-view"') &&
    !sidebarSource.includes('aria-label="Agent trace"'),
  "Agent trace must live inside the Inspector Debug drawer and use distinct accessible semantic icons"
);
assert(
  sidebarSource.includes('aria-label="Create project"') &&
    sidebarSource.includes('title="Create project"') &&
    inspectorSource.includes('aria-label={`Preview ${artifactName(displayPath)}${') &&
    inspectorSource.includes('title={`Preview ${artifactName(displayPath)}${') &&
    composerSource.includes('aria-label={showStop ? "Stop agent" : agentActive ? "Queue message" : "Send message"}') &&
    composerSource.includes('title={showStop ? "Stop" : agentActive ? "Queue" : "Send"}'),
  "Icon-only operations must expose accessible hover labels"
);
assert(
  sidebarSource.includes("Menu.new") &&
    sidebarSource.includes("menu.popup") &&
    sidebarSource.includes("onContextMenu"),
  "Session actions must use the native Tauri context menu"
);
assert(
  !styles.includes(".session-menu"),
  "Session actions must not retain a custom-drawn menu"
);
assert(settingsPageSource.includes('id: "runtime"'), "Settings must expose Runtime");
assert(settingsPageSource.includes('id: "permissions"'), "Settings must expose Permissions");
assert(
  settingsPageSource.includes('id: "about"') &&
    settingsPageSource.includes('data-settings-group="about"') &&
    settingsPageSource.includes("runtime?.appVersion") &&
    settingsPageSource.includes("about-app"),
  "Settings must expose an About page with the packaged runtime version"
);
assert(
  settingsPageSource.indexOf('id: "personalization"') <
      settingsPageSource.indexOf('id: "about"') &&
    settingsPageSource.includes('data-settings-group="personalization"') &&
    settingsPageSource.includes("What should Cindx call you?") &&
    settingsPageSource.includes("Response tone") &&
    settingsPageSource.includes("Response length") &&
    settingsPageSource.includes("Appearance") &&
    settingsPageSource.includes('["light", "Light", Sun]') &&
    settingsPageSource.includes('["dark", "Dark", Moon]') &&
    settingsPageSource.includes('["system", "System", Monitor]') &&
    tauriBridge.includes("getPersonalizationConfig") &&
    tauriBridge.includes("savePersonalizationConfig") &&
    rustLib.includes("personalized_agent_instructions") &&
    rustLib.includes('app_data_root().join("personalization.json")'),
  "Personalization must persist and affect agent prompts before About"
);
assert(
  settingsUiSource.includes("personalizationSaveQueueRef") &&
    settingsUiSource.includes("updatePersonalizationDraft") &&
    settingsPageSource.includes("flushPersonalization(false)") &&
    rustLib.includes("The user's preferred name is") &&
    rustLib.includes("If the user asks what their name is"),
  "Personalization changes must auto-save and expose the preferred name as agent identity context"
);
assert(
  styles.includes('.app-shell[data-sidebar-open="false"] .sidebar-pane-toggle') &&
    styles.includes(':root[data-theme="dark"] .skill-url-row') &&
    styles.includes(':root[data-theme="dark"] .integration-row') &&
    styles.includes(':root[data-theme="dark"] .tool-schema') &&
    styles.includes(':root[data-theme="dark"] .source-preview') &&
    styles.includes(':root[data-theme="dark"] .knowledge-graph-canvas'),
  "Collapsed navigation and Settings surfaces must remain visible in dark mode"
);
assert(
  /\.composer-input-shell \{[^}]*background-clip: padding-box;[^}]*border: 1px solid var\(--border\);[^}]*border-radius: 24px;[^}]*box-shadow: none;[^}]*\}/.test(
    styles
  ) &&
    /\.composer textarea \{[^}]*background: transparent;[^}]*border-radius: 0;[^}]*\}/.test(styles) &&
    !/\.composer-toolbar \{[^}]*border-radius:/.test(styles) &&
    !styles.includes(".composer-input-shell > textarea:first-child") &&
    !styles.includes(':root[data-theme="dark"] .composer-input-shell'),
  "Composer must use one crisp 24px shell instead of stitched child corners"
);
assert(
  /\.settings-view,\s*\.schedule-view \{[^}]*--form-control-radius: 10px;[^}]*--form-control-focus-ring: 0 0 0 2px rgb\(37 99 235 \/ 14%\);/.test(
    styles
  ) &&
    /\.schedule-field input,[\s\S]*?\.schedule-field textarea \{[^}]*background-clip: padding-box;[^}]*border-radius: var\(--form-control-radius\);[^}]*box-shadow: none;/.test(
      styles
    ) &&
    /\.provider-form input,[\s\S]*?\.tool-runner textarea \{[^}]*background-clip: padding-box;[^}]*border-radius: var\(--form-control-radius\);[^}]*outline: 0;[^}]*box-shadow: none;/.test(
      styles
    ) &&
    /\.skill-url-row:focus-within \{[^}]*border-color: var\(--accent\);[^}]*box-shadow: var\(--form-control-focus-ring\);/.test(
      styles
    ),
  "Settings and Schedule fields must share one crisp rounded shell and blue focus treatment"
);
assert(
  styles.includes("--settings-element-radius: var(--form-control-radius)") &&
    /\.settings-view button \{[^}]*border-radius: var\(--settings-element-radius\);/.test(styles) &&
    /\.settings-view :is\([\s\S]*?\.workspace-folder-selector,[\s\S]*?\.tool-output,[\s\S]*?\.settings-inline-error[\s\S]*?\) \{[^}]*border-radius: var\(--settings-element-radius\);/.test(
      styles
    ) &&
    /\.settings-view \.about-app img,\s*\.settings-saved-toast \{[^}]*border-radius: 10px;/.test(
      styles
    ),
  "Settings rectangular controls and surfaces must share one rounded geometry"
);
assert(
  appSource.includes('className="settings-saved-toast"') &&
    appSource.includes("showSettingsSaved();") &&
    desktopControllerSource.includes('showSettingsSaved("Personalization saved")') &&
    desktopControllerSource.includes('showSaved("Provider verified and configured")') &&
    (desktopControllerSource.match(/showSaved\(/g)?.length ?? 0) === 5 &&
    appSource.includes("<CheckCircle2 aria-hidden=\"true\" />") &&
    styles.includes(".settings-saved-toast") &&
    styles.includes("color: #2f9e64;"),
  "Every explicit Settings save must show one green SVG Saved toast"
);
assert(
  settingsPageSource.includes("<dt>Created by</dt>") &&
    settingsPageSource.includes("<dd>Dale, 2026</dd>"),
  "About must show the project credit instead of a generic platform label"
);
assert(
  rustLib.includes("WindowEvent::CloseRequested") &&
    rustLib.includes("RunEvent::ExitRequested") &&
    rustLib.includes("NSAlert::new") &&
    rustLib.includes("setShowsSuppressionButton(true)") &&
    rustLib.includes("Don't ask again") &&
    rustLib.includes("skip_quit_confirmation=true") &&
    cargoToml.includes("objc2-app-kit"),
  "macOS quit paths must use a native AppKit confirmation with persistent suppression"
);
assert(
  settingsPageSource.includes('id: "agent"') &&
    settingsPageSource.includes("Agent instructions") &&
    settingsPageSource.includes("Custom instructions") &&
    settingsPageSource.includes("cannot replace permission or") &&
    settingsPageSource.includes("providerDraft.agentSystemPrompt") &&
    tauriBridge.includes("agentSystemPrompt: string") &&
    rustLib.includes("agent_system_prompt_hex") &&
    rustLib.includes("model_request_for_turn_with_context") &&
    agentRuntimeSource.includes("compose_base_agent_system_prompt") &&
    coreAgentPrompt.includes("Cindx core contract") &&
    coreAgentPrompt.includes("Verify the requested result with direct evidence") &&
    coreAgentPrompt.includes("fenced `mermaid` block") &&
    coreAgentPrompt.includes("fenced `mindmap` block") &&
    coreAgentPrompt.includes("renders it with Markmap"),
  "Settings must persist lower-priority Agent instructions without replacing the core contract"
);
assert(!styles.includes("artifact-sidebar"), "Legacy artifact sidebar styles must be removed");
assert(
  inspectorSource.includes("inspector-outputs") &&
    inspectorSource.includes("inspector-debug") &&
    inspectorSource.includes("readArtifactPreview") &&
    inspectorSource.includes('sandbox=""') &&
    inspectorSource.includes("useState(false)") &&
    !inspectorSource.includes("outputArtifacts[0]?.path") &&
    inspectorSource.includes("selectOutput(artifact.path)") &&
    inspectorSource.includes('aria-label="Close preview"'),
  "Inspector must default to an output file list, open previews on demand, and keep Debug collapsed"
);
assert(
  inspectorSource.includes('aria-label={outputPreviewFullscreen ? "Exit full screen" : "Show full screen"}') &&
    inspectorSource.includes('aria-label="Open with default app"') &&
    inspectorSource.includes("createPortal(outputPreview, document.body)") &&
    inspectorSource.includes("sessionArtifact(step") &&
    inspectorSource.includes("isInternalRuntimePath") &&
    inspectorSource.includes("step.metadata.result_artifact_path") &&
    inspectorSource.includes('step.toolName === "file.write"') &&
    inspectorSource.includes("getAgentSessionOutputs") &&
    inspectorSource.includes("outputHistoryBySession") &&
    inspectorSource.includes("mergeOutputArtifacts") &&
    inspectorSource.includes("const authoritative = mergeOutputArtifacts(resolved)") &&
    inspectorSource.includes("currentRunOutputs") &&
    inspectorSource.includes("}, [sessionId]);") &&
    artifactProjectionSource.includes('get("result_artifact_path")') &&
    artifactProjectionSource.includes("is_internal_runtime_path(logical_path)") &&
    tauriBridge.includes('invoke<AgentOutputArtifactView[]>("get_agent_session_outputs"') &&
    rustLib.includes("get_agent_session_outputs") &&
    rustLib.includes("agent_output_artifacts_from_events") &&
    toolsSource.includes('join("output-history")') &&
    toolsSource.includes('metadata.insert(\n            "artifact_path"') &&
    !inspectorSource.includes("contextCheckpoint?.artifacts.forEach") &&
    !inspectorSource.includes("ragSources.forEach") &&
    !inspectorSource.includes("browserObservations.forEach") &&
    appWorkspaceProjectionSource.includes(
      "agentTraceState?.sessionId === activeSession.id"
    ) &&
    appSource.includes("sessionTraceSteps={activeSessionTraceSteps}") &&
    styles.includes('.inspector-output-detail[data-fullscreen="true"]') &&
    styles.includes("position: fixed;") &&
    styles.includes("z-index: 100;"),
  "Output previews must retain session history, preserve file versions, and stay scoped to the active session"
);
assert(
  /\.inspector-debug::before \{[\s\S]*?height: calc\(var\(--inspector-debug-panel-height\) \+ 44px\);[\s\S]*?clip-path: inset\(calc\(100% - 44px\) 0 0 0\);[\s\S]*?backdrop-filter: blur\(18px\) saturate\(1\.08\);/.test(styles) &&
    /\.inspector-debug\[data-open="true"\]::before \{[\s\S]*?clip-path: inset\(0\);/.test(styles) &&
    /\.inspector-debug-body \{[\s\S]*?right: 0;[\s\S]*?bottom: 44px;[\s\S]*?left: 0;[\s\S]*?background: transparent;[\s\S]*?border: 0;[\s\S]*?clip-path: inset\(100% 0 0 0\);[\s\S]*?clip-path 190ms/.test(styles) &&
    /\.inspector-debug\[data-open="true"\] \.inspector-debug-body \{[\s\S]*?clip-path: inset\(0\);[\s\S]*?clip-path 230ms/.test(styles) &&
    !/\.inspector-debug-body \{[^}]*transform:/.test(styles) &&
    inspectorSource.includes('data-state={debugOpen ? "expanded" : "collapsed"}') &&
    inspectorSource.includes("<ChevronDown />") &&
    !inspectorSource.includes("ChevronUp") &&
    /<Bug[^>]*\/>\s*<strong>Debug<\/strong>\s*<span[\s\S]*?className="inspector-debug-chevron"[\s\S]*?data-state=\{debugOpen \? "expanded" : "collapsed"\}[\s\S]*?<\/span>/.test(
      inspectorSource
    ) &&
    /\.inspector-debug-chevron > svg \{[\s\S]*?transform: rotate\(180deg\);[\s\S]*?transition: transform 180ms/.test(
      styles
    ) &&
    /\.inspector-debug-chevron\[data-state="expanded"\] > svg \{[\s\S]*?transform: rotate\(0deg\);/.test(
      styles
    ) &&
    /\.inspector-debug-toggle \{[\s\S]*?grid-template-columns: 13px max-content 11px;[\s\S]*?align-items: center;/.test(
      styles
    ),
  "The debug drawer must open as one synchronized surface with a centered, state-correct trailing chevron"
);
assert(
  tauriBridge.includes('invoke<ArrayBuffer>("read_artifact_image"') &&
    rustLib.includes("async fn read_artifact_image") &&
    rustLib.includes("read_artifact_image_bytes") &&
    rustLib.includes("tauri::ipc::Response::new") &&
    artifactImagePreviewHookSource.includes("URL.createObjectURL") &&
    artifactImagePreviewHookSource.includes("readArtifactPreview") &&
    artifactImagePreviewHookSource.includes('preview.kind !== "image"') &&
    artifactImagePreviewCacheSource.includes("byteLimit") &&
    rustLib.includes("canonical_path.starts_with(&canonical_root)") &&
    rustLib.includes("MAX_ARTIFACT_IMAGE_BYTES"),
  "Artifact image previews must use bounded raw IPC outside the command thread"
);
assert(
  tauriBridge.includes('invoke<void>("open_artifact"') &&
    rustLib.includes("fn open_artifact") &&
    rustLib.includes('std::process::Command::new("open")') &&
    rustLib.includes("validated_workspace_artifact_path(&state, &path)"),
  "Default-app artifact opening must reuse workspace path validation"
);
assert(
  inspectorSource.includes("revealArtifact") &&
    inspectorSource.includes('title="Show in Finder"') &&
    inspectorSource.includes("inspector-output-row") &&
    tauriBridge.includes('invoke<void>("reveal_artifact"') &&
    rustLib.includes("fn reveal_artifact") &&
    rustLib.includes('command.arg("-R").arg(&canonical_path)') &&
    styles.includes(".inspector-output-reveal") &&
    !inspectorSource.includes('<span title={displayPath}>{displayPath}</span>'),
  "Outputs must use a compact path-free file list with a validated Finder reveal action"
);
assert(
  appSource.includes("async function handleExportAgentTrace()") &&
    appSource.includes("const next = await exportAgentTraceJsonl(activeSession?.id)") &&
    appSource.includes("if (next.exportPath)") &&
    appSource.includes("await revealArtifact(next.exportPath)"),
  "Exporting agent trace JSONL must reveal the exported file in the system file browser"
);
assert(
  inspectorSource.includes("const [outputsOpen, setOutputsOpen] = useState(true)") &&
    /className="inspector-output-count"[\s\S]*?className="inspector-output-toggle"/.test(
      inspectorSource
    ) &&
    inspectorSource.includes("data-expanded={outputsOpen}") &&
    inspectorSource.includes("!outputsOpen ? null : selectedOutput") &&
    /\.inspector-outputs > header \.inspector-output-chevron \{[\s\S]*?transform: rotate\(0deg\);[\s\S]*?transition: transform 180ms/.test(
      styles
    ) &&
    /\.inspector-outputs > header \.inspector-output-chevron\[data-expanded="true"\] \{[\s\S]*?transform: rotate\(180deg\);/.test(
      styles
    ),
  "Outputs must show an upward-pointing chevron when expanded and a downward-pointing chevron when collapsed"
);
assert(!tauriBridge.includes("apiKeyPreview"), "Provider state must not expose API key suffixes");
assert(settingsPageSource.includes("<ModelSelect"), "Provider models must use select controls");
assert(desktopUiSource.includes("listProviderModels"), "Provider settings must load the remote model catalog");
assert(
  appSource.includes("providerStatusText(providerReadiness)") &&
    composerSource.includes("providerSubmissionPreflight(providerReadiness)") &&
    composerSource.indexOf("if (!providerPreflight.allowSubmit)") <
      composerSource.indexOf("onSend(prompt)") &&
    composerSource.indexOf("onSend(prompt)") < composerSource.indexOf('onChange("")') &&
    composerSource.includes("Configure Models") &&
    appSource.includes('openSettingsCategory("models")') &&
    settingsModelsPanelSource.includes("providerSettingsError") &&
    settingsModelsPanelSource.includes('role="alert"') &&
    settingsModelsPanelSource.includes("handleReloadProviderState") &&
    settingsModelsPanelSource.includes("Retry provider settings") &&
    settingsModelsPanelSource.includes(
      '          </div>\n        )}\n        {providerSettingsError && ('
    ) &&
    rustLib.includes("ready: config.is_ready()"),
  "Provider first-use gate must preserve drafts, expose Models recovery, and use Rust readiness"
);
assert(
  settingsPageSource.includes('label="Conductor"') &&
    settingsPageSource.includes("providerDraft.conductorModel") &&
    tauriBridge.includes("conductorModel: string") &&
    rustLib.includes("conductor_model: String") &&
    rustLib.includes("model_for_conductor"),
  "Models settings must persist and use a dedicated Conductor model"
);
assert(
  workspaceChromeSource.includes("context-usage") &&
    /\.topbar-actions \{[\s\S]*?gap: 12px;/.test(styles) &&
    /\.context-usage \{[\s\S]*?width: 132px;/.test(styles) &&
    /\.context-usage progress \{[\s\S]*?width: 124px;[\s\S]*?height: 2px;[\s\S]*?border-radius: var\(--radius-pill\);/.test(
      styles
    ),
  "Topbar must expose compact rounded context usage with breathing room before runtime state"
);
assert(
  workspaceChromeSource.includes('activeView !== "settings"') &&
    styles.includes(".topbar-title") &&
    styles.includes(".topbar-actions") &&
    !styles.includes("--titlebar-content-offset-y"),
  "Chat title and status controls must remain hidden in Settings and optically aligned elsewhere"
);
assert(
  settingsPageSource.includes('className="settings-tabs"') &&
    settingsPageSource.includes('aria-current={settingsCategory === category.id ? "page" : undefined}') &&
    styles.includes(".settings-tabs > button.active span") &&
    !settingsPageSource.includes('aria-label="Back to Settings"'),
  "Settings must keep a persistent vertical category tab list with a bold active title"
);
assert(
  workspaceChromeSource.includes("window-workspace-header") &&
    !desktopUiSource.includes('className="topbar"'),
  "Session title, context usage, and runtime status must be integrated into the window titlebar"
);
assert(
  settingsPageSource.includes("Default effort") &&
    settingsPageSource.includes("Cindx Fast") &&
    settingsPageSource.includes("Cindx Auto") &&
    settingsPageSource.includes("Cindx Pro"),
  "Provider settings must expose the three Cindx effort modes"
);
assert(settingsPageSource.includes("Archived sessions"), "Settings must expose archived session recovery");
assert(
  (settingsPageSource.match(/className="runtime-state-value"/g)?.length ?? 0) === 3 &&
    settingsPageSource.includes('data-state={sidecarState?.autoConfigure ? "auto" : "manual"}') &&
    /\.runtime-state-value\[data-state="ready"\],[\s\S]*?color: #2f9e64;/.test(styles),
  "Runtime Ready and Auto states must use green SVG status indicators"
);
assert(
  settingsPageSource.includes("skillRefreshTurn") &&
    settingsPageSource.includes("providerModelsRefreshTurn") &&
    (settingsPageSource.match(/settings-refresh-turn/g)?.length ?? 0) >= 2 &&
    styles.includes("@keyframes settings-refresh-turn"),
  "Settings refresh actions must replay a one-turn icon animation for every click"
);
assert(
  agentSkillsSource.includes("BUILTIN_SKILL_CREATOR_ID") &&
    agentSkillsSource.includes("include_str!") &&
    agentSkillsSource.includes("unwrap_or(true)") &&
    builtinSkillCreator.includes("name: Claude Code Skill Creator") &&
    builtinSkillCreator.includes("SKILL.md contract"),
  "The trusted Claude Code Skill Creator must ship inside the Rust skill catalog"
);
assert(
  !settingsPageSource.includes("Local folder") &&
    settingsPageSource.includes(".skill package") &&
    desktopUiSource.includes("installSkillUrl") &&
    !rustLib.includes("install_skill_directory") &&
    rustLib.includes("install_skill_package") &&
    rustLib.includes("install_skill_url") &&
    agentSkillsSource.includes("install_skill_archive") &&
    agentSkillsSource.includes("enclosed_name"),
  "Skills settings must install safe packaged and HTTPS skills"
);
assert(
  fs.existsSync(path.join(root, "apps/desktop/src/assets/fonts/Borel-Regular.ttf")) &&
    fs.existsSync(path.join(root, "apps/desktop/src/assets/fonts/Borel-OFL.txt")) &&
    styles.includes('font-family: "Borel", cursive') &&
    styles.includes("background: #2563eb") &&
    composerSource.includes("Send"),
  "Cindx branding and the send action must retain their bundled type and blue accent"
);
assert(
    settingsPageSource.includes("Knowledge sources") &&
    settingsPageSource.includes("knowledge-results") &&
    desktopControllerSource.includes("runRagOperation") &&
    desktopControllerSource.includes("ensureWorkspaceKnowledge()") &&
    tauriBridge.includes('invoke<Phase7State>("ensure_workspace_knowledge")') &&
    rustLib.includes("fn ensure_workspace_knowledge") &&
    rustLib.includes("fn prepare_manual_rag_snapshot") &&
    rustLib.includes("ensure_workspace_knowledge_index(") &&
    !settingsPageSource.includes("Test retrieval") &&
    tauriBridge.includes("if (isTauriRuntime()) throw error"),
  "Knowledge search must auto-index, expose results inline, and surface real Tauri errors"
);
assert(
  settingsPageSource.includes("Web search API") &&
    settingsPageSource.includes("Save web search") &&
    settingsPageSource.includes("registered-tools-details") &&
    tauriBridge.includes('invoke<WebSearchConfigState>("save_web_search_config"') &&
    rustLib.includes("fn save_web_search_config(") &&
    rustLib.includes("web-search.conf") &&
    rustLib.includes("with_workspace_tools_and_services") &&
    toolsSource.includes("fetch_search_api") &&
    toolsSource.includes('stdin_bytes: Option<&[u8]>') &&
    toolsSource.includes('command.args(["-q", "-L"])') &&
    toolsSource.includes('command.args(["--max-redirs", "0"])') &&
    toolsSource.includes('["--proto", "=https", "--proto-redir", "=https"]') &&
    toolsSource.includes('command.args(["--header", "@-"])') &&
    toolsSource.includes("web search endpoints with an API key must use HTTPS") &&
    !toolsSource.includes('.arg(format!("Authorization: Bearer'),
  "Tools settings must persist a private custom web search API and expand built-in tool details"
);
assert(
  settingsPageSource.includes('label="Image generation"') &&
    settingsPageSource.includes("Image API endpoint") &&
    settingsPageSource.includes("provider-endpoint-check") &&
    desktopUiSource.includes("validateImageEndpoint") &&
    settingsPageSource.includes('emptyLabel="Not configured"') &&
    styles.includes("grid-template-columns: minmax(0, 1fr) 18px") &&
    styles.includes(".provider-endpoint-check") &&
    !/\.provider-endpoint-check \{[^}]*position: absolute;/.test(styles) &&
    tauriBridge.includes("imageModel: string") &&
    tauriBridge.includes("imageEndpoint: string") &&
    tauriBridge.includes('invoke<ImageEndpointValidationState>("validate_image_endpoint"') &&
    rustLib.includes("image_endpoint") &&
    rustLib.includes("async fn validate_image_endpoint") &&
    rustLib.includes("ImageGenerationConfig") &&
    toolsSource.includes('"image.generate"') &&
    modelProviderSource.includes('"/images/generations"') &&
    modelProviderSource.includes("pub fn validate_endpoint") &&
    modelProviderSource.includes("DashScopeMultimodal"),
  "Models settings must persist an image model and expose the image.generate agent tool"
);
assert(
  rustLib.includes("DASHSCOPE_DEFAULT_EMBEDDING_MODEL") &&
    rustLib.includes("fn embedding_model_for_provider(") &&
    rustLib.includes("fn index_workspace_with_cloud_fallback(") &&
    rustLib.includes('"local-fallback".to_string()') &&
    rustLib.includes('"embedding_fallback_error"'),
  "Workspace indexing must migrate incompatible embedding defaults and retain a local fallback"
);
assert(
  settingsPageSource.includes("Pending Reviews") &&
    settingsPageSource.includes("permission-review-list") &&
    appSource.includes("handleResolvePermissionReview") &&
    appSource.includes("handleIgnorePermissionReview") &&
    settingsPageSource.includes("review.sessionName") &&
    tauriBridge.includes("getPermissionReviewState") &&
    rustLib.includes("async fn get_permission_review_state(") &&
    rustLib.includes("let store = open_app_read_store()?") &&
    appSource.includes("let inFlight = false;") &&
    rustLib.includes("struct PermissionReviewItem"),
  "Permission settings must present actionable reviews with source session context"
);
assert(
  styles.includes(".advanced-settings summary::-webkit-details-marker") &&
    settingsPageSource.includes("function SettingsChevron") &&
    styles.includes(".settings-disclosure-chevron"),
  "Expandable settings must use a consistent trailing chevron"
);
assert(
  settingsPageSource.includes('className="settings-sidebar"') &&
    settingsPageSource.includes('className="settings-detail"') &&
    settingsPageSource.includes('{settingsCategory === "skills" && (') &&
    settingsPageSource.includes('{settingsCategory === "permissions" && (') &&
    settingsPageSource.includes('{settingsCategory === "knowledge" && (') &&
    !settingsPageSource.includes("settings-index") &&
    styles.includes("grid-template-columns: 168px minmax(0, 760px)") &&
    /\.settings-app-return \{[\s\S]*?top: -12px;/.test(styles),
  "Settings must use persistent left tabs and right-side details"
);
assert(
  sessionTitleServiceSource.includes("persist_completed_conversation_title") &&
    sessionTitleServiceSource.includes("spawn_semantic_session_title_refinement") &&
    sessionTitleServiceSource.includes("semantic_session_title") &&
    sessionTitleServiceSource.includes("session_title_refinement_needed") &&
    sessionTitleServiceSource.includes("generated_session_title_copies_conversation") &&
    sessionTitleServiceSource.includes("session_title_refinement_sessions") &&
    sessionTitleServiceSource.includes("SessionTitleState::Manual") &&
    sessionTitleServiceSource.includes(
      "session.title_state = SessionTitleState::Automatic"
    ) &&
    !sessionTitleServiceSource.includes("automatic_conversation_title") &&
    sessionTitleServiceSource.includes("app.emit_session_title_updated(") &&
    (rustLib.match(/persist_completed_conversation_title/g) || []).length >= 2 &&
    appSource.includes("subscribeToSessionTitleUpdates") &&
    tauriBridge.includes('listen<string>("session-title-updated"'),
  "Session titles must refine semantically, retry safely, and never overwrite manual names"
);
assert(
  styles.includes(".lucide-check") &&
    styles.includes(".lucide-circle-check") &&
    styles.includes("color: var(--accent) !important") &&
    styles.includes("--sidebar-glass: linear-gradient(") &&
    styles.includes("rgba(255, 255, 255, 0.91) 0%") &&
    styles.includes("rgba(255, 255, 255, 0.8) 100%") &&
    styles.includes("background: var(--sidebar-glass)") &&
    /\.window-toolbar-panel-left \{[^}]*background-position: left top;[^}]*background-size: 100% 100vh;/.test(
      styles
    ) &&
    /\.sidebar \{[^}]*background-position: left calc\(0px - var\(--titlebar-height\)\);[^}]*background-size: 100% 100vh;/.test(
      styles
    ) &&
    /\.project-item \{[^}]*padding-left: 12px;/.test(styles) &&
    !/\.window-toolbar-panel-left \{[^}]*backdrop-filter:/.test(styles) &&
    !/\.sidebar \{[^}]*backdrop-filter:/.test(styles) &&
    rustLib.includes("install_macos_sidebar_material") &&
    rustLib.includes("window_vibrancy::apply_vibrancy") &&
    rustLib.includes("NSVisualEffectMaterial::Sidebar") &&
    rustLib.includes("NSVisualEffectState::Active") &&
    rustLib.includes("MACOS_SIDEBAR_MATERIAL_TAG") &&
    rustLib.includes("set_sidebar_material_width") &&
    tauriBridge.includes('invoke<void>("set_sidebar_material_width"') &&
    appSource.includes('activeView === "settings" || !sidebarOpen ? 0 : sidebarWidth') &&
    styles.includes("--project-selection: rgba(210, 211, 214, 0.78)") &&
    styles.includes("--session-selection: rgba(220, 221, 224, 0.82)") &&
    !/\.project-row\.active \{[^}]*box-shadow:/.test(styles) &&
    !/\.session-item\.active \{[^}]*box-shadow:/.test(styles) &&
    inspectorSource.includes("PackageOpen") &&
    inspectorSource.includes("<PackageOpen aria-hidden=\"true\" />"),
  "Checkmarks, native sidebar material, and the Outputs heading icon must retain their visual treatment"
);
assert(
  appSource.includes("busy={projectSessionBusy}") &&
    appSource.includes("busySessionIds") &&
    composerDraftsSource.includes("const [drafts, setDrafts]") &&
    !appSource.includes("agentBusy"),
  "Running one session must not disable navigation to other sessions"
);
const projectSelectionBlock =
  appSource.match(
    /async function handleSelectProject[\s\S]*?(?=\n  async function handleSelectSession)/
  )?.[0] ?? "";
assert(
  projectSelectionBlock.includes(
    "if (projectId === projectSessionState?.activeProjectId) return;"
  ) &&
    projectSelectionBlock.includes("setProjectSessionState((current) =>") &&
    !projectSelectionBlock.includes("setProjectSessionBusy"),
  "Project selection must update optimistically without freezing the session list"
);
assert(
  tauriBridge.includes("export async function runAgentTask(") &&
    tauriBridge.includes("sessionId: string") &&
    tauriBridge.includes("currentTime: currentAgentTimeContext()") &&
    rustLib.includes("project_session_metadata_for_session") &&
    rustLib.includes("event.metadata.get(\"session_id\")"),
  "Agent commands and event boundaries must remain isolated by session"
);
assert(
  agentTaskCommandSource.includes("fn persisted_agent_policy_from_active_events(") &&
    (agentTaskCommandSource.match(
      /persisted_agent_policy_from_active_events\(&active_events\)\?/g
    )?.length ?? 0) === 2,
  "Runtime retries must decode persisted agent policy strictly before resuming"
);
assert(
  composerSource.includes("const EFFORT_OPTIONS") &&
    composerSource.includes('label: "Cindx Fast"') &&
    composerSource.includes('description: "One model for quick, focused tasks"') &&
    composerSource.includes('label: "Cindx Auto"') &&
    composerSource.includes('description: "Routes each request by complexity"') &&
    composerSource.includes('label: "Cindx Pro"') &&
    composerSource.includes(
      'description: "Learns from Auto and continuously improves"'
    ) &&
    composerSource.includes('className="composer-effort-menu"') &&
    composerSource.includes('role="listbox"') &&
    composerSource.includes('role="option"') &&
    appWorkspaceProjectionSource.includes(
      "normalizedSessionEffort(activeSession?.effort)"
    ) &&
    appSource.includes("handleSessionEffortChange") &&
    appSource.includes("setSessionEffort(sessionId, effort)") &&
    !appSource.includes('useState<AgentEffort>("auto")') &&
    tauriBridge.includes('export type AgentEffort = "fast" | "auto" | "pro"') &&
    tauriBridge.includes("effort: AgentEffort") &&
    tauriBridge.includes("export async function setSessionEffort") &&
    tauriBridge.includes("currentTime: currentAgentTimeContext(), effort, attachments") &&
    orchestratorSource.includes("pub enum AgentPolicy") &&
    orchestratorSource.includes("pub fn parse_persisted(value: &str) -> Option<Self>") &&
    rustLib.includes("fn set_session_effort(") &&
    rustLib.includes("fn update_session_effort(") &&
    rustLib.includes("session_effort_updates_only_the_selected_session") &&
    rustLib.includes('"agent_effort".to_string()') &&
    rustLib.includes("persisted_agent_policy_from_active_events"),
  "Composer effort must persist per session, default independently, and survive retries and traces"
);
assert(
  tauriNativeTypesSource.includes("export type NativeRuntimeStatus = {") &&
    tauriNativeTypesSource.includes("agentRunBudgets: AgentRunBudgets;") &&
    tauriTypesSource.includes("export type RuntimeStatus = {") &&
    tauriTypesSource.includes("agentRunBudgets: AgentRunBudgets | null") &&
    agentRunBudgetModelSource.includes("budgets?.[effort]") &&
    appSource.includes(
      "optimisticRunBudgetPatch(runtime?.agentRunBudgets, agentEffort)"
    ) &&
    !appShellModelSource.includes("runBudgetForEffort") &&
    rustLib.includes("agent_run_budgets: AgentRunBudgetsView") &&
    rustLib.includes('fast: agent_run_budget_view("fast")') &&
    rustLib.includes('auto: agent_run_budget_view("auto")') &&
    rustLib.includes('pro: agent_run_budget_view("pro")'),
  "Agent run budgets must flow from the Rust runtime catalog into optimistic UI state"
);
assert(
  tauriBridge.includes("function currentAgentTimeContext()") &&
    rustLib.includes("normalized_current_time_context") &&
    rustLib.includes("agent_runtime_context_for_run") &&
    rustLib.includes("collaboration_system_prompt_for_run") &&
    rustLib.includes("Current date and time: {current_time}"),
  "Every new agent turn must receive an automatically computed current time"
);
assert(
  rustLib.includes(
    "session_permission_grant_only_covers_the_same_capability"
  ) &&
    permissionServiceSource.includes("list_permission_audits_for_session") &&
    !permissionServiceSource.includes("fn permission_capability_matches") &&
    agentCorePermissionPolicySource.includes("pub fn permission_capability_matches") &&
    agentCorePermissionPolicySource.includes("granted.risk == requested.risk") &&
    agentCorePermissionPolicySource.includes("granted.action == requested.action") &&
    agentCorePermissionPolicySource.includes("permission_capability_metadata_matches") &&
    permissionServiceSource.includes("permission_can_allow_session") &&
    agentStorageSource.includes("idx_permission_requests_session_capability_key") &&
    agentStorageSource.includes("backfill_permission_capability_keys") &&
    permissionServiceSource.includes('request.metadata.contains_key("command")') &&
    desktopAgentToolRuntimeSource.includes("agent_session_permission_granted(") &&
    agentCorePermissionPolicySource.includes(
      "request.risk == PermissionRisk::Destructive"
    ) &&
    agentCorePermissionPolicySource.includes(
      'session_reusable == Some("true")'
    ) &&
    rustLib.includes(
      ".filter(|pending| permission_capability_matches(&request, pending))"
    ) &&
    rustLib.includes("this permission can only be allowed once") &&
    composerSource.includes("approvalInputSummary") &&
    composerSource.includes("pendingApproval.canAllowSession") &&
    composerSource.includes("Reuse only this exact command in this session"),
  "Allow session must reuse only the same capability and never cover destructive tools"
);
assert(
  !rustLib.includes("mod agent_grounding_policy") &&
    !rustLib.includes("mod agent_loop_service") &&
    agentRuntimeGroundingToolsSource.includes(
      "pub fn pin_prompt_evidence_tools"
    ) &&
    agentRuntimeGroundingToolsSource.includes(
      "pub fn pin_evidence_scope_tools"
    ) &&
    agentRuntimeGroundingToolsSource.includes(
      "pub fn tool_matches_evidence_scope"
    ) &&
    agentRuntimeGroundingPolicySource.includes("pub fn prompt_evidence_scopes") &&
    agentRuntimeModelTransportSource.includes(
      "pub fn model_transport_retry_delay"
    ) &&
    agentRuntimeModelTransportSource.includes("pub struct ModelStreamProgress") &&
    agentLoopContractRuntimeSource.includes("use agent_runtime::{") &&
    (agentLoopContractRuntimeSource.includes(
      "pin_prompt_evidence_tools(run_context"
    ) ||
      agentLoopContractRuntimeSource.includes(
        "pin_evidence_scope_tools(&completion_intent.evidence_scopes"
      )),
  "Grounding and model transport policy must remain portable agent-runtime ownership"
);
assert(
  rustLib.includes("execute_agent_tool_invocation") &&
    rustLib.includes("drop(store);") &&
    appSource.includes("markSessionBusy"),
  "Agent tool execution must release the shared event-store lock"
);
assert(
  /join\("Library"\)\s*\.join\("Application Support"\)\s*\.join\("Cindx"\)/.test(
    rustLib
  ) &&
    appBootstrapSource.includes("persistent state unavailable; startup aborted") &&
    appBootstrapSource.includes("show_native_startup_failure(&message)") &&
    !appBootstrapSource.includes("SqliteStore::in_memory()") &&
    appBootstrapSource.indexOf("let mut store = match open_app_store()") <
      appBootstrapSource.indexOf("tauri::Builder::default()") &&
    rustLib.includes("pub(crate) fn open_app_store_at(database_path: &Path)") &&
    rustLib.includes("ExitCode::FAILURE") &&
    rustLib.includes("install_startup_panic_log") &&
    cargoToml.includes(
      'rustls = { version = "0.23.42", default-features = false, features = ["ring"] }'
    ) &&
    appBootstrapSource.includes("fn install_rustls_crypto_provider()") &&
    appBootstrapSource.indexOf("install_startup_panic_log();") <
      appBootstrapSource.indexOf("install_rustls_crypto_provider();") &&
    appBootstrapSource.indexOf("install_rustls_crypto_provider();") <
      appBootstrapSource.indexOf("migrate_legacy_app_data()") &&
    appBootstrapSource.includes(
      "rustls_crypto_provider_installation_is_idempotent"
    ) &&
    rustLib.includes('std::env::var("CINDX_STARTUP_PROBE")') &&
    ciWorkflow.includes("Probe clean-machine startup") &&
    ciWorkflow.includes("Probe persistent-state failure") &&
    !rustLib.includes('workspace_root().join(".cindx").join("state.sqlite3")'),
  "Installed apps must use user-scoped data and fail closed before desktop services start"
);
assert(
  rustLib.includes("EVENT_REDACTION_MARKER_FILE") &&
    appBootstrapSource.includes("if event_redaction_pending {") &&
    !appBootstrapSource.includes("event_redaction_pending && persistent_store") &&
    rustLib.includes("event_redaction_marker_records_completed_migration"),
  "Legacy event redaction must be a versioned one-time startup migration"
);
assert(
  appSource.includes("agentStateUnchanged") &&
    appSource.includes("agentTraceUnchanged") &&
    appSource.includes("sessionLifecycleRefreshRef") &&
    appSource.includes("sessionLifecycleRefreshRef.current === refreshRequest"),
  "Agent polling must preserve unchanged React state references"
);
assert(
  rustLib.includes("get_agent_state_revision") &&
    rustLib.includes("AGENT_HISTORY_INITIAL_PAGE_SIZE: usize = 120") &&
    tauriBridge.includes("getAgentStateRevision") &&
    appSource.includes("agentStateRevisionsRef") &&
    appSource.includes("agentStateRevisionsRef.current.set(state.sessionId") &&
    sessionThreadSource.includes("const RunProgressStatus = memo") &&
    styles.includes("content-visibility: auto"),
  "Long sessions must avoid full-state polling and repeated offscreen rendering"
);
assert(
  rustLib.includes('AGENT_RECOVERY_SCHEMA: &str = "cindx.agent-recovery.v1"') &&
    rustLib.includes("AgentRecoveryEnvelope") &&
    agentRecoveryServiceSource.includes("claim_agent_recovery_envelope") &&
    agentRecoveryServiceSource.includes("recovery_safe_transcript") &&
    agentRecoveryServiceSource.includes("reconcile_interrupted_agent_runs") &&
    agentRecoveryServiceSource.includes('"Agent task paused"') &&
    rustLib.includes('"continuation_replay"') &&
    agentRecoveryServiceSource.includes(
      "AgentRecoveryReason::AppRestartedWaitingForPermission"
    ) &&
    tauriBridge.includes('| "paused"'),
  "Long agent runs must recover durably without replaying unknown tool outcomes"
);
assert(
  /let \(task_state, resource_snapshot\)\s*=\s*match resolve_agent_recovery_identity[\s\S]*?load_matching_agent_runtime_snapshot[\s\S]*?load_matching_agent_resource_snapshot[\s\S]*?agent_recovery_metadata_with_task_state[\s\S]*?append_event\([\s\S]*?delete_persisted_agent_runtime_snapshot/.test(
    agentRecoveryServiceSource
  ) &&
    agentRecoveryServiceSource.includes("already_recovered_wait") &&
    agentRecoveryServiceSource.includes("stale agent runtime snapshot cleanup unavailable") &&
    agentRuntimeSnapshotSource.includes("PersistedAgentRuntimeSnapshot") &&
    agentRuntimeSnapshotSource.includes("event_revision_by_metadata") &&
    agentRuntimeSnapshotSource.includes("prompt_fingerprint") &&
    agentRuntimeSnapshotSource.includes("load_matching_agent_runtime_snapshot") &&
    agentRuntimeSnapshotSource.includes("capture_persistable_agent_task_state") &&
    rustLib.includes("runtime_snapshot_matches_the_redacted_durable_projection"),
  "Restart recovery must transfer task state into the recovery event and retire stale run snapshots"
);
assert(
  sessionOutputCacheStoreSource.includes("SESSION_OUTPUT_CACHE_LIMIT: usize = 32") &&
    sessionOutputCacheSource.includes("event_revision_by_metadata") &&
    sessionOutputCacheSource.includes("list_by_task_and_metadata_after") &&
    sessionOutputCacheSource.includes("merge_agent_output_delta") &&
    sessionOutputCacheSource.includes("rebuild_agent_output_artifacts") &&
    rustLib.includes("session_output_cache: SessionOutputCache") &&
    rustLib.includes("session_output_cache.remove_many(session_ids)") &&
    rustLib.includes(
      "delete_read_model(AGENT_RUNTIME_SNAPSHOT_READ_MODEL_NAMESPACE, session_id)"
    ),
  "Session outputs must survive restarts through revisioned reconstruction and invalidate with session runtime state"
);
assert(
  agentMemorySource.includes('MEMORY_LEDGER_SCHEMA: &str = "cindx.memory-ledger.v6"') &&
    agentMemorySource.includes("USER_REQUIREMENT_EVIDENCE_SCHEMA") &&
    agentMemorySource.includes("user_requirement_evidence") &&
    agentMemorySource.includes("mod extraction;") &&
    agentMemorySource.includes("mod ledger;") &&
    agentMemorySource.includes("mod recall;") &&
    agentMemorySource.includes("mod requirement_scope;") &&
    agentMemorySource.includes("mod semantic;") &&
    agentMemorySource.includes("superseded_by") &&
    agentMemorySource.includes("extract_durable_memories") &&
    agentMemorySource.includes("merge_memory_records") &&
    agentMemorySource.includes("recall_memories_at") &&
    agentMemorySource.includes("fuse_memory_recalls_at") &&
    agentMemorySource.includes("record_memory_observed_uses") &&
    agentMemorySource.includes("observed_use_count") &&
    agentMemorySource.includes("Memory does not override the current user request") &&
    rustLib.includes('AGENT_MEMORY_READ_MODEL_NAMESPACE: &str = "agent-memory-v2"') &&
    rustLib.includes("load_project_memory_ledger") &&
    rustLib.includes("recall_project_memory_for_prompt") &&
    rustLib.includes("schedule_project_memory_vector_refresh") &&
    rustLib.includes("memory_lancedb_database_path_for") &&
    memoryProjectionRuntimeSource.includes("memory_ledger_is_intrinsically_valid") &&
    !memoryProjectionRuntimeSource.includes(".event_by_id(") &&
    rustLib.includes("MemoryStatsView") &&
    rustLib.includes('"Project memory recalled"') &&
    rustLib.includes('"Project memory utilization measured"') &&
    rustLib.includes(
      "let recall_memory = !matches!(decision.memory.policy, MemoryRecallPolicy::None)"
    ) &&
    rustLib.includes("project_memory_hybrid_fallback") &&
    agentMemorySource.includes("validate_semantic_memory_batch") &&
    agentMemorySource.includes("source_event_ids") &&
    agentMemorySource.includes(
      "recalled_memory_is_serialized_as_quoted_json_data"
    ) &&
    rustLib.includes("delete_project_memory") &&
    settingsPageSource.includes('aria-label="Project memory stats"'),
  "Project memory must be durable, deduplicated, explainable, trust-scoped, and deleted with its project"
);
assert(
  settingsPageFileSource.includes("<SettingsMemoryPanel") &&
    settingsPageFileSource.includes("useMemorySettingsController") &&
    settingsMemoryPanelSource.includes('title="Active"') &&
    settingsMemoryPanelSource.includes("function MemoryGroup(") &&
    settingsMemoryPanelSource.includes("memory-settings-disclosure") &&
    settingsMemoryPanelSource.includes('className="settings-disclosure-chevron"') &&
    !settingsMemoryPanelSource.includes("collapsible?: boolean") &&
    settingsMemoryPanelSource.includes('title="Disabled / inactive"') &&
    settingsMemoryPanelSource.includes('title="Needs review"') &&
    settingsMemoryPanelSource.includes("Save as project requirement") &&
    settingsMemoryPanelSource.includes('role="alert"') &&
    settingsMemoryPanelSource.includes("Pinning changes recall order only; it does not change trust") &&
    memoryManagementModelSource.includes('| "superseded"') &&
    memoryManagementModelSource.includes("expectedItemRevision") &&
    memoryManagementModelSource.includes("expectedContentSha256") &&
    memorySettingsControllerSource.includes("projectIdRef.current === targetProjectId") &&
    tauriBridgeImplementation.includes(
      'invoke<projectMemory.ProjectMemoryState>("get_project_memory_state", { projectId })'
    ) &&
    tauriBridgeImplementation.includes(
      'invoke<projectMemory.ProjectMemoryState>("update_project_memory", { input })'
    ),
  "Project memory Settings must keep memory groups collapsed by default and controls scoped, reviewable, stale-safe, and trust preserving"
);
assert(
  rustLib.includes("WORKSPACE_KNOWLEDGE_CACHE_TTL") &&
    rustLib.includes("cached_workspace_knowledge_snapshot_for") &&
    rustLib.includes("graph_store: Option<Arc<FileGraphStore>>") &&
    rustLib.includes("entry.snapshot_if_current(&active_index_path)") &&
    rustLib.includes("snapshot.graph_store.as_deref()") &&
    rustLib.includes("graph_state_for_snapshot(&snapshot") &&
    rustLib.includes("invalidate_workspace_knowledge_cache") &&
    rustLib.includes('timed_retrieval_channel("graph_walk"') &&
    rustLib.includes("let mut channels = std::thread::scope") &&
    rustLib.includes("let graph_store = if include_graph {") &&
    rustLib.includes("cached_graph_store.or(opened_graph_store.as_ref())") &&
    rustLib.includes("let graph_seeds = graph_walk_seed_results") &&
    rustLib.includes("channels.push(timed_retrieval_channel") &&
    rustLib.includes("search_lancedb_index(") &&
    tauriBridge.includes("indexCacheHit") &&
    settingsPageSource.includes('"index cached"'),
  "Knowledge retrieval must reuse a bounded cache, parallelize direct channels, then graph-walk from their seeds"
);
assert(
  rustLib.includes("agent_trace_role_summaries") &&
    rustLib.includes("AgentTraceRoleSummaryView") &&
    rustLib.includes('"first_token_latency_ms"') &&
    tauriBridge.includes("AgentTraceRoleSummary") &&
    inspectorSource.includes('aria-label="Model role activity"') &&
    inspectorSource.includes("TTFT"),
  "Agent trace must expose the real model, latency, evidence, and completion status for each collaboration role"
);
assert(
  orchestratorSource.includes("pub fn pareto_front") &&
    orchestratorSource.includes("prompt_profile") &&
    orchestratorSource.includes("prompt_genome") &&
    promptEvolutionSource.includes("pub fn mutations") &&
    promptEvolutionSource.includes("pub fn next_generation") &&
    promptEvolutionSource.includes("learned_mutation_from_response") &&
    promptEvolutionSource.includes("evaluate_prompt_convergence") &&
    promptEvolutionSource.includes("pub fn reward(&self)") &&
    promptEvolutionSource.includes("pub fn group_relative_reward(&self)") &&
    promptEvolutionSource.includes("pub enum PromptToolPolicy") &&
    promptEvolutionSource.includes("pub enum PromptRetryPolicy") &&
    promptEvolutionSource.includes("PromptEvaluationMode::PairedExecution") &&
    promptEvolutionSource.includes("PromptEvaluationMode::ReplayExecution") &&
    promptEvolutionSource.includes("prompt_promotion_confidence") &&
    promptEvolutionSource.includes("wilson_lower_bound") &&
    promptEvolutionSource.includes("average_step_credit") &&
    promptEvolutionSource.includes("pareto_selection_does_not_treat_token_cost_as_intelligence") &&
    promptEvolutionSource.includes("format_valid_rate < 1.0") &&
    rustLib.includes("pareto_search_teacher_v2") &&
    rustLib.includes("prompt_evolution_enabled") &&
    rustLib.includes("prompt_evolution_evaluation_for_run") &&
    rustLib.includes('"prompt_evolution_mutation"') &&
    rustLib.includes("PROMPT_EVOLUTION_STAGNATION_PATIENCE") &&
    rustLib.includes("PROMPT_EVOLUTION_SHADOW_INTERVAL") &&
    promptEvolutionReadModelSource.includes("is_agent_run_terminal") &&
    promptEvolutionReadModelSource.includes("AgentRunEvent::from_event") &&
    rustLib.includes("evaluate_prompt_evolution") &&
    rustLib.includes("Conductor prompt profile selected") &&
    rustLib.includes("PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS") &&
    rustLib.includes("PROMPT_EVOLUTION_BACKGROUND_BATCH_LIMIT") &&
    rustLib.includes("prompt_direct_profile_evidence_counts") &&
    rustLib.includes("prompt_rollout_counterpart") &&
    rustLib.includes("opponent_profile_id.as_deref()") &&
    rustLib.includes("let current_counts = prompt_direct_profile_evidence_counts") &&
    rustLib.includes("let challenger_counts = prompt_direct_profile_evidence_counts") &&
    rustLib.includes("PROMPT_EVOLUTION_OFFLINE_MIN_CASES") &&
    rustLib.includes("prompt_offline_dataset") &&
    rustLib.includes("select_prompt_offline_case") &&
    rustLib.includes('"Conductor offline dataset selected"') &&
    rustLib.includes("BACKGROUND_WORK_IDLE_GRACE_MS: u64 = 30_000") &&
    promptEvolutionWorkerSource.includes("wait_for_foreground_agent_idle") &&
    read("apps/desktop/src-tauri/src/semantic_memory_worker.rs").includes(
      "wait_for_foreground_agent_idle"
    ) &&
    read("apps/desktop/src-tauri/src/semantic_memory_runtime.rs").includes(
      "foreground_agent_should_preempt"
    ) &&
    rustLib.includes("schedule_prompt_pairwise_evaluation") &&
    rustLib.includes("bounded_evolution") &&
    rustLib.includes("prompt_objective") &&
    rustLib.includes("conductor_directive.as_deref()") &&
    rustLib.includes("evaluate_prompt_candidate_pair") &&
    rustLib.includes("execute_prompt_workflow_candidate") &&
    rustLib.includes("struct PromptWorkflowExecution") &&
    rustLib.includes("prompt_replay_case") &&
    rustLib.includes('"Conductor pairwise evaluation"') &&
    rustLib.includes("PromptEvaluationMode::PairedExecution") &&
    rustLib.includes("PromptEvaluationMode::ReplayExecution") &&
    rustLib.includes("reconcile_prompt_rollout") &&
    rustLib.includes("next_prompt_canary_stage") &&
    rustLib.includes("prompt_canary_degraded") &&
    rustLib.includes("prompt_rollout_transition_is_valid") &&
    rustLib.includes('next.status == "promoted"') &&
    rustLib.includes(
      "previous.canary_profile_id.as_deref() == Some(next.stable_profile_id.as_str())"
    ) &&
    rustLib.includes(
      "snapshot.stable_profile_id == previous.stable_profile_id"
    ) &&
    rustLib.includes(
      "PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION: u32 = 8"
    ) &&
    rustLib.includes("snapshot.genome.id == rollout.stable_profile_id") &&
    rustLib.includes("promoted_prompt_rollout_without_a_valid_frozen_profile_is_ignored") &&
    rustLib.includes(
      "prompt_rollout_replay_rejects_forged_stable_canary_and_status"
    ) &&
    rustLib.includes(
      "prompt_rollout_transition_accepts_exact_stages_and_atomic_promotion"
    ) &&
    rustLib.includes("stable_prompt_rollout_uses_the_evidence_bound_frozen_genome") &&
    rustLib.includes('"Conductor prompt rollout updated"') &&
    rustLib.includes('"evaluation_required"') &&
    tauriBridge.includes("setPromptEvolutionEnabled") &&
    tauriBridge.includes("averageRelativeReward") &&
    tauriBridge.includes("averageStepCredit") &&
    tauriBridge.includes("promotionConfidence") &&
    tauriBridge.includes("canaryPercent") &&
    settingsPageSource.includes('if (category === "tools") return <Wrench aria-hidden="true" />;') &&
    /<Wrench size=\{17\} aria-hidden="true" \/>\s*<h2>Tools<\/h2>/.test(settingsPageSource) &&
    /<Dna size=\{17\} aria-hidden="true" \/>\s*<h2>Genetic Pareto<\/h2>/.test(settingsPageSource) &&
    settingsPageSource.includes("Genetic Pareto") &&
    settingsPageSource.includes("Candidate harnesses execute in an isolated arena before promotion") &&
    /direct\s+stable-versus-challenger Wilson gate controls staged canary rollout/.test(
      settingsPageSource
    ) &&
    settingsPageSource.includes("Evaluating in background") &&
    settingsPageSource.includes("Rollout by effort") &&
    settingsPageSource.includes("Candidate profiles") &&
    settingsPageSource.includes("prompt-evolution-table") &&
    settingsPageSource.includes("prompt-evolution-summary") &&
    settingsPageSource.includes("Rollbacks") &&
    styles.includes(".prompt-evolution-table") &&
    !settingsPageSource.includes("prompt-evolution-efforts") &&
    !settingsPageSource.includes("prompt-evolution-profiles"),
  "Conductor workflows must run executable harness evolution with confidence-gated canary rollout"
);
assert(
  orchestratorSource.includes('"cindx.prompt-learning-eligibility.v1"') &&
    orchestratorSource.includes('"cindx.prompt-dataset-identity.v1"') &&
    orchestratorSource.includes('"cindx.prompt-evaluation-attempt.v1"') &&
    orchestratorSource.includes("PromptLearningQualificationInput") &&
    rustLib.includes("PromptEvaluationAttemptGuard::start") &&
    orchestratorSource.includes("is_strict_matched_evidence") &&
    rustLib.includes(
      "frozen prompt dataset is incomplete; refusing cohort substitution"
    ) &&
    promptEvolutionHotStateSource.includes(
      "PROMPT_EVALUATION_ATTEMPT_RETENTION: usize = 1_024"
    ) &&
    localBuildScript.includes("CINDX_SOURCE_REVISION"),
  "Prompt learning must remain eligibility-gated, cohort-bound, matched, auditable, and source-versioned"
);

const nonGrayColors = [...styles.matchAll(/#([0-9a-fA-F]{6})(?![0-9a-fA-F])/g)]
  .map((match) => match[1].toLowerCase())
  .filter(
    (hex) =>
      ![
        "2563eb",
        "1d4ed8",
        "245fae",
        "2f9e64",
        "3974c8",
        "39b96b",
        "60a5fa",
        "8ab8f7",
        "9fe3b0",
        "9f2d25",
        "b5d2fa",
        "bfdbfe",
        "dbeafe",
        "e05b5b",
        "e5c7c4",
        "e3efff",
        "eef5ff",
        "eef6ff",
        "eff6ff",
        "f7f9fc",
        "fca5a5",
        "fff4f2"
      ].includes(hex)
  )
  .filter((hex) => hex.slice(0, 2) !== hex.slice(2, 4) || hex.slice(2, 4) !== hex.slice(4, 6));
assert(
  nonGrayColors.length === 0,
  "Desktop theme must remain grayscale except for brand, message, and session-state accents"
);
assert(settingsPageSource.includes("Pending Reviews"), "App must render pending permission reviews");
assert(
  !settingsPageSource.includes("<h2>Orchestration</h2>") &&
    !settingsPageSource.includes("Manual workflow test") &&
    tauriBridge.includes('invoke<Phase6State>("run_orchestration"'),
  "Manual orchestration must stay out of user settings while remaining available to diagnostics"
);
assert(settingsPageSource.includes("Provider"), "App must render provider UI");
assert(settingsPageSource.includes("Save workspace"), "App must render workspace save action");
assert(
  appSource.includes("handlePickWorkspace") &&
    settingsPageSource.includes("workspace-folder-selector") &&
    appSource.includes("pickWorkspaceFolder"),
  "Workspace settings must use the native folder selector"
);
assert(
  sidebarSource.includes("Folders") &&
    sidebarSource.includes("nav-heading-with-icon") &&
    styles.includes(".nav-heading-with-icon"),
  "Projects heading must render an aligned SVG icon"
);
assert(
  settingsPageSource.includes("Connect provider"),
  "App must render provider verification and connect action"
);
assert(settingsPageSource.includes("Run tool"), "App must render the Phase 5 tool runner");
assert(settingsPageSource.includes("Index workspace"), "App must render the Phase 7 RAG index action");
assert(desktopUiSource.includes("answerWithRag"), "App must render the Phase 7 RAG answer flow");
assert(settingsPageSource.includes("Search web"), "App must render the Phase 8 web search action");
assert(desktopUiSource.includes("runBrowserTool"), "App must render the Phase 8 browser flow");
assert(
  settingsPageSource.includes('onRunBrowserTool("browser.tabs")') &&
    settingsPageSource.includes('onRunBrowserTool("browser.select_tab")'),
  "Browser settings must expose tab listing and selection"
);
assert(appSource.includes("getRuntimeStatus"), "App must call the runtime bridge");
assert(
  !settingsPageSource.includes("Request review") &&
    settingsPageSource.includes("Approve once"),
  "Permission settings must review real pending actions instead of creating mock requests"
);

assert(
  tauriBridge.includes('invoke<unknown>("get_runtime_status")') &&
    tauriBridge.includes("decodeNativeRuntimeStatus("),
  "Frontend bridge must invoke get_runtime_status"
);
assert(
  tauriBridge.includes('invoke<unknown>("save_workspace_root"') &&
    tauriBridge.includes("decodeNativeRuntimeStatus("),
  "Frontend bridge must invoke save_workspace_root"
);
assert(
  tauriBridge.includes('invoke<string | null>("pick_workspace_folder"'),
  "Frontend bridge must invoke pick_workspace_folder"
);
assert(
  tauriBridge.includes('invoke<Phase3State>("get_phase3_state")'),
  "Frontend bridge must invoke get_phase3_state"
);
assert(
  tauriBridge.includes('invoke<Phase3State>("request_mock_permission")'),
  "Frontend bridge must invoke request_mock_permission"
);
assert(
  tauriBridge.includes('invoke<Phase3State>("resolve_permission"'),
  "Frontend bridge must invoke resolve_permission"
);
assert(
  tauriBridge.includes('invoke<Phase4State>("get_phase4_state")'),
  "Frontend bridge must invoke get_phase4_state"
);
assert(
  tauriBridge.includes('invoke<Phase4State>("save_provider_config"'),
  "Frontend bridge must invoke save_provider_config"
);
assert(
  tauriBridge.includes('invoke<ProviderModelsState>("list_provider_models"'),
  "Frontend bridge must invoke list_provider_models"
);
assert(
  tauriBridge.includes('invoke<Phase4State>("send_model_prompt"'),
  "Frontend bridge must invoke send_model_prompt"
);
assert(
  tauriBridge.includes('listen<ModelStreamDelta>("model-stream-delta"'),
  "Frontend bridge must listen for model stream deltas"
);
assert(
  tauriBridge.includes('invoke<Phase5State>("get_phase5_state")'),
  "Frontend bridge must invoke get_phase5_state"
);
assert(
  tauriBridge.includes('invoke<Phase5State>("run_tool"'),
  "Frontend bridge must invoke run_tool"
);
assert(
  tauriBridge.includes('invoke<Phase5State>("resolve_tool_permission"'),
  "Frontend bridge must invoke resolve_tool_permission"
);
assert(
  tauriBridge.includes('invoke<Phase6State>("get_phase6_state")'),
  "Frontend bridge must invoke get_phase6_state"
);
assert(
  tauriBridge.includes('invoke<Phase6State>("run_orchestration"'),
  "Frontend bridge must invoke run_orchestration"
);
assert(
  tauriBridge.includes('invoke<Phase7State>("get_phase7_state")'),
  "Frontend bridge must invoke get_phase7_state"
);
assert(
  tauriBridge.includes('invoke<Phase7State>("index_workspace_rag"'),
  "Frontend bridge must invoke index_workspace_rag"
);
assert(
  tauriBridge.includes('invoke<Phase7State>("search_rag"'),
  "Frontend bridge must invoke search_rag"
);
assert(
  tauriBridge.includes('invoke<Phase7State>("answer_with_rag"'),
  "Frontend bridge must invoke answer_with_rag"
);
assert(
  tauriBridge.includes('invoke<Phase8State>("get_phase8_state")'),
  "Frontend bridge must invoke get_phase8_state"
);
assert(
  tauriBridge.includes('invoke<Phase8State>("run_browser_tool"'),
  "Frontend bridge must invoke run_browser_tool"
);
assert(
  tauriBridge.includes('invoke<Phase8State>("resolve_browser_permission"'),
  "Frontend bridge must invoke resolve_browser_permission"
);
assert(rustLib.includes("#[tauri::command]"), "Rust bridge must expose a Tauri command");
assert(rustLib.includes("fn get_runtime_status("), "Rust bridge command is missing");
assert(rustLib.includes("fn save_workspace_root("), "Workspace save command is missing");
assert(
  rustLib.includes("fn pick_workspace_folder(") && rustLib.includes("NSOpenPanel"),
  "Native workspace folder picker command is missing"
);
assert(rustLib.includes("fn get_phase3_state("), "Phase 3 state command is missing");
assert(
  rustLib.includes("fn request_mock_permission("),
  "Phase 3 mock permission command is missing"
);
assert(rustLib.includes("fn resolve_permission("), "Phase 3 resolution command is missing");
assert(rustLib.includes("fn get_phase4_state("), "Phase 4 state command is missing");
assert(rustLib.includes("fn save_provider_config("), "Phase 4 provider config command is missing");
assert(rustLib.includes("async fn list_provider_models("), "Provider model catalog command is missing");
assert(rustLib.includes("async fn run_agent_task("), "Agent task must not block the IPC thread");
assert(rustLib.includes("fn rename_project("), "Project rename command is missing");
assert(rustLib.includes("fn rename_session("), "Session rename command is missing");
assert(
  composerSource.includes('aria-label="Attach files"') &&
    tauriBridgeImplementation.includes('export { stageAgentAttachments } from "./attachmentIpc.ts"') &&
    attachmentIpcSource.includes('invoke<AgentAttachment>("stage_agent_attachment", bytes') &&
    attachmentIpcSource.includes("file.arrayBuffer()") &&
    attachmentIpcSource.includes("batchFileSizes") &&
    attachmentIpcSource.includes('invoke<void>("abort_agent_attachment_batch"') &&
    rustLib.includes("async fn stage_agent_attachment(") &&
    rustLib.includes("tauri::ipc::InvokeBody::Raw(bytes)") &&
    rustLib.includes("struct AttachmentUploadBatches") &&
    rustLib.includes("attachment batch index was uploaded more than once") &&
    rustLib.includes("cleanup_staged_attachment_paths(paths)") &&
    rustLib.includes("stage_raw_agent_attachment") &&
    rustLib.includes("validated_attachment_path"),
  "Composer attachments must use bounded raw IPC and stage inside the active project"
);
assert(
  rustLib.includes("model_message_from_event") &&
    rustLib.includes('"model_content"') &&
    rustLib.includes('"recovery_prompt"') &&
    rustLib.includes("agent_recovery_prompt_from_active_events") &&
    rustLib.includes("attachment_message_separates_display_and_model_content") &&
    rustLib.includes("recovery_identity_stays_on_root_prompt_after_steer"),
  "Attachment and steer recovery must separate display, model, and root prompt identity"
);
assert(
    rustLib.includes("append_visual_reference_message") &&
    rustLib.includes('"image_paths"') &&
    modelProviderSource.includes("image_url") &&
    modelProviderSource.includes("image_data_url"),
  "Browser and computer screenshots must return to the model as visual references"
);
assert(rustLib.includes("fn fork_session("), "Session fork command is missing");
assert(rustLib.includes("fn archive_session("), "Session archive command is missing");
assert(rustLib.includes("fn restore_session("), "Session restore command is missing");
assert(rustLib.includes("fn delete_session("), "Session delete command is missing");
assert(
    rustLib.includes("async fn resolve_agent_permission(") &&
    rustLib.includes("Agent task resumed after permission") &&
    rustLib.includes("permission_checkpoint_message_boundary") &&
    rustLib.includes(
      "permission_checkpoint_boundary_uses_the_matching_blocked_event"
    ) &&
    rustLib.includes(
      "permission_boundary_uses_unique_permission_id_when_call_ids_repeat"
    ),
  "Permission resolution must resume the agent off the IPC thread"
);
assert(
  rustLib.includes("redact_persisted_events") && rustLib.includes("redact_sensitive_text"),
  "Persisted agent history must be redacted before display or replay"
);
assert(
  toolsSource.includes("reject_sensitive_read_path") &&
    toolsSource.includes(".cindx/provider.conf"),
  "Workspace tools must block local credential reads"
);
assert(
  ragSource.includes("skips_local_credentials_when_indexing_workspace"),
  "RAG indexing must exclude local credential files"
);
assert(
  ragSource.includes("DEFAULT_EMBEDDING_BATCH_SIZE: usize = 20") &&
    ragSource.includes("texts.chunks(DEFAULT_EMBEDDING_BATCH_SIZE)") &&
    ragSource.includes("external_embeddings_are_requested_in_provider_safe_batches"),
  "RAG cloud embeddings must stay within the provider-safe batch limit"
);
assert(
  rustLib.includes("run_planned_retrieval(") &&
    rustLib.includes("retrieval_plan: &WorkspaceRetrievalPlan") &&
    rustLib.includes('timed_retrieval_channel("semantic_rag"') &&
    rustLib.includes('timed_retrieval_channel("graph_recall"') &&
    rustLib.includes('timed_retrieval_channel("graph_walk"') &&
    rustLib.includes('timed_retrieval_channel("file_search"') &&
    rustLib.includes("plan.channels") &&
    orchestratorSource.includes("WorkspaceRetrievalPlan") &&
    orchestratorSource.includes("WorkspaceRetrievalChannel") &&
    rustLib.includes("fuse_retrieval_channels") &&
    rustLib.includes("prepare_agent_knowledge_context") &&
    ragSource.includes("search_chunks_semantic") &&
    ragSource.includes("search_chunks_literal"),
  "Knowledge retrieval must execute the conductor-selected channels, fuse results, and feed the agent"
);
assert(
  settingsPageSource.includes("Graph Explorer") &&
    knowledgeGraphSource.includes('aria-label="Workspace knowledge graph"') &&
    knowledgeGraphSource.includes('from "d3-force"') &&
    knowledgeGraphSource.includes("forceSimulation(positioned)") &&
    knowledgeGraphSource.includes("forceLink<PositionedNode, SimulationEdge>") &&
    knowledgeGraphSource.includes("graph.edges.filter") &&
    knowledgeGraphSource.includes("Math.min(6.6") &&
    knowledgeGraphSource.includes('className="knowledge-graph-scene" ref={sceneRef}') &&
    knowledgeGraphSource.includes('ref={canvasRef}') &&
    knowledgeGraphSource.includes('"IntersectionObserver" in window') &&
    knowledgeGraphSource.includes('document.visibilityState !== "hidden"') &&
    knowledgeGraphSource.includes("observer?.disconnect()") &&
    knowledgeGraphSource.includes("const activeNodeIds = useMemo") &&
    knowledgeGraphSource.includes("window.requestAnimationFrame(update)") &&
    knowledgeGraphSource.includes('line.setAttribute("x1", String(source.x))') &&
    knowledgeGraphSource.includes('line.setAttribute("y2", String(target.y))') &&
    knowledgeGraphSource.includes('window.matchMedia("(prefers-reduced-motion: reduce)")') &&
    knowledgeGraphSource.includes("data-related={connected || undefined}") &&
    knowledgeGraphSource.includes("data-muted={Boolean(activeId) && !related") &&
    !knowledgeGraphSource.includes("data-highlighted") &&
    !knowledgeGraphSource.includes("knowledge-graph-node-halo") &&
    styles.includes(".knowledge-graph-node-label") &&
    styles.includes('.knowledge-graph-edges line[data-muted="true"]') &&
    /\.knowledge-graph-edges line\[data-related="true"\] \{[^}]*stroke: #4f4f4f;/.test(
      styles
    ) &&
    styles.includes(".knowledge-graph-node") &&
    styles.includes("will-change: transform") &&
    !styles.includes("animation: knowledge-graph-float") &&
    !/\.knowledge-graph-node\[data-active="true"\][\s\S]*?transform: scale/.test(styles),
  "Knowledge settings must expose a restrained force-directed graph with connected ambient node motion"
);
assert(
  settingsPageSource.includes("const KnowledgeGraph = lazy(() =>") &&
    settingsPageSource.includes("knowledgeGraphOpen ? (") &&
    appShellModelSource.includes("BACKGROUND_AGENT_POLL_INTERVAL_MS = 5_000") &&
    appSource.includes('document.visibilityState === "hidden"') &&
    appSource.includes('document.addEventListener("visibilitychange"'),
  "Heavy graph code and active-session polling must pause or defer while their surfaces are not visible"
);
assert(
  appSource.includes("onStreamDone={handleAgentStreamDone}") &&
    sessionThreadSource.includes("onStreamDone: (sessionId: string) => boolean | Promise<boolean>") &&
    modelStreamSubscriptionSource.includes("finishAndSynchronize") &&
    rustLib.includes("let completed_state = match terminal_commit") &&
    rustLib.includes("RunTerminalCommit::Committed(state) => state") &&
    rustLib.includes(
      'emit_agent_stream_delta(app, &delivery_request_id, session_id, "", true, false, None);'
    ) &&
    rustLib.includes("Ok(AgentCompletionOutcome::Completed(completed_state))") &&
    /AgentCompletionOutcome::Completed\(agent_state\)\s*=>\s*\{\s*return Ok\(AgentLoopExecutionOutcome::Finished\(agent_state\)\)/.test(
      rustLib
    ),
  "A committed terminal stream event must refresh the active session without waiting for polling"
);
assert(
  rustLib.includes('"elapsed_ms".to_string()') &&
    rustLib.includes('"model_calls".to_string()') &&
    rustLib.includes('"tool_calls".to_string()') &&
    rustLib.includes('"last_stage".to_string()'),
  "Completed agent runs must persist performance counters for regression analysis"
);
assert(
  sessionThreadProjectionSource.includes('event.label === "Model started"') &&
    sessionThreadProjectionSource.includes('event.label === "Model finished"') &&
    rustLib.includes('"Model started"') &&
    rustLib.includes('"Model finished"'),
  "Internal model lifecycle events must stay in trace storage without appearing in chat"
);
assert(
  /:root\s*\{[\s\S]*?font-size: 14px;/.test(styles) &&
    /\.brand-name\s*\{[\s\S]*?font-size: 16px;/.test(styles) &&
    /\.composer-error,\s*\.composer-continuation\s*\{[\s\S]*?border-radius: var\(--radius-lg\);/.test(
      styles
    ) &&
    !/\.composer-(?:error|continuation)\s*\{[^}]*border-left:/.test(styles),
  "App typography must be one step larger while the Cindx brand stays fixed and run notices use rounded bars"
);
assert(
  rustLib.includes("context_checkpoint_path_for_session") &&
    sessionContextServiceSource.includes("prepare_session_history_context") &&
    sessionContextServiceSource.includes("SessionCompactionPlan") &&
    rustLib.includes('CONTEXT_COMPACTION_VERSION: &str = "hybrid_v4_prefix_events"') &&
    sessionContextServiceSource.includes("ContextCheckpointManifest") &&
    sessionContextServiceSource.includes("covered_prefix_sha256") &&
    sessionContextServiceSource.includes("context_events_for_covered_history_prefix") &&
    sessionContextServiceSource.includes('"covered_events"') &&
    sessionContextServiceSource.includes("recent_history_start") &&
    sessionContextServiceSource.includes("if can_reuse_checkpoint") &&
    sessionContextServiceSource.includes("} else if plan.should_compact {") &&
    !sessionContextServiceSource.includes(
      "if plan.should_compact && !can_reuse_checkpoint"
    ) &&
    agentMemorySource.includes("conversation_memory_to_markdown") &&
    sessionContextServiceSource.includes("Session context restored for agent run") &&
    rustLib.includes("event_matches_context"),
  "Context compaction must preserve session-scoped operational and conversational memory"
);
assert(
  rustLib.includes("prepare_agent_execution(") &&
    rustLib.includes("plan_agent_run(") &&
    rustLib.includes("run_adaptive_collaboration(") &&
    collaborationServiceSource.includes("struct AdaptiveCollaborationSpec") &&
    collaborationServiceSource.includes("struct CollaborationCompletion") &&
    collaborationWorkerRuntimeSource.includes("IsolatedWorkerRuntime::new(") &&
    adaptiveCollaborationFinalizationSource.includes("fn finalize_adaptive_collaboration(") &&
    orchestratorSource.includes(
      'AGENT_RUN_DECISION_SCHEMA: &str = "cindx.agent-run-decision.v1"'
    ) &&
    orchestratorSource.includes("pub struct AgentRunDecisionHarness") &&
    orchestratorSource.includes("pub struct ConductorHarness") &&
    orchestratorSource.includes("pub fn planning_prompt(&self)") &&
    orchestratorSource.includes("pub fn repair_prompt(") &&
    orchestratorSource.includes("pub fn parse_plan(") &&
    rustLib.includes("CONDUCTOR_MAX_ATTEMPTS") &&
    rustLib.includes("complete_collaboration_worker_with_tools(") &&
    rustLib.includes('"isolated_evidence_v1"') &&
    agentRuntimeSource.includes("evidence_worker_tools") &&
    parallelExecutionSource.includes("MAX_GLOBAL_MODEL_WORKERS: usize = 12") &&
    parallelExecutionSource.includes("BoundedParallelExecutor") &&
    parallelExecutionSource.includes("run_model_jobs_until_anytime_quorum_interruptible") &&
    rustLib.includes('"conductor_plan"') &&
    rustLib.includes('format!("worker_{}", step_index + 1)') &&
    collaborationServiceSource.includes(
      '("access_list".to_string(), spec.access.join(","))'
    ) &&
    rustLib.includes("recover_adaptive_worker(") &&
    rustLib.includes("quality_gate_adaptive_output(") &&
    rustLib.includes("append_single_model_policy_guidance(") &&
    orchestratorSource.includes("MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5") &&
    orchestratorSource.includes("MAX_ADAPTIVE_WORKFLOW_AGENTS: usize = 3") &&
    orchestratorSource.includes("adaptive_workflow_step_budget") &&
    orchestratorSource.includes("adaptive_workflow_layers") &&
    orchestratorSource.includes("must only access earlier steps") &&
    rustLib.includes('"workflow_ir".to_string()') &&
    orchestratorSource.includes('WORKFLOW_IR_SCHEMA: &str = "cindx.workflow.v1"') &&
    orchestratorSource.includes(
      'WORKFLOW_REVISION_SCHEMA: &str = "cindx.workflow.revision.v1"'
    ) &&
    orchestratorSource.includes("MAX_WORKFLOW_PLAN_REVISIONS") &&
    rustLib.includes("validate_and_apply_revision(") &&
    rustLib.includes("Collaboration workflow planned") &&
    rustLib.includes("Tool evidence ledger") &&
    rustLib.includes("collaboration_step_result(") &&
    rustLib.includes('"evidence_count"'),
  "Primary agent must use a bounded, tool-capable, conductor-planned and revisable workflow"
);
assert(
  orchestratorSource.includes("evaluate_routing_cases") &&
    orchestratorSource.includes("evaluate_routing_telemetry") &&
    orchestratorSource.includes("QualityRubricScore") &&
    benchmarkSource.includes('AGENT_BENCHMARK_SCHEMA: &str = "cindx.agent-benchmark.v1"') &&
    benchmarkSource.includes("AgentBenchmarkObservation") &&
    benchmarkSource.includes("observed_mode_thresholds") &&
    benchmarkSuite.cases.length >= 50 &&
    benchmarkSuite.cases.length <= 100 &&
    benchmarkBaseline.minimum_auto_contract_pass_rate === 1 &&
    evaluationLabSource.includes("Cindx agent benchmark") &&
    evaluationLabSource.includes("quality=not_observed") &&
    agentEvaluationDoc.includes("## Evidence Levels") &&
    agentEvaluationDoc.includes("## Current Real-World Findings") &&
    qualityGateManifest.schema === "cindx.quality-gates.v1" &&
    qualityGateManifest.profiles["ci-contract"].includes("routing-contract") &&
    qualityGateRunner.includes("cindx.quality-gate-report.v1") &&
    qualityGateDoc.includes("deterministic green build") &&
    ciWorkflow.includes("run-quality-gates.mjs --profile ci-contract") &&
    ciWorkflow.includes("Upload quality reports") &&
    ciWorkflow.includes("target/agent-benchmark-report.json"),
  "CI must run the versioned offline benchmark and retain auditable quality guidance"
);
assert(
  JSON.stringify(qualityGateManifest.profiles["shipping-performance"]) ===
    JSON.stringify(shippingPerformanceGateIds) &&
    shippingPerformanceGateIds.every(
      (id) =>
        qualityGateManifest.profiles.performance.includes(id) &&
        qualityGateManifest.profiles.full.includes(id)
    ) &&
    !qualityGateManifest.profiles["shipping-performance"].some((id) =>
      [
        "routing-contract",
        "evaluation-foundation",
        "agent-arena-contract",
        "memory-contract"
      ].includes(id)
    ) &&
    shippingPerformanceGateIds.every((id) => {
      const gate = qualityGateById.get(id);
      const proof = shippingPerformanceProofs.get(id);
      const isFrontend = id === "frontend-streaming-markdown-scaling";
      return (
        gate &&
        proof &&
        gate.category === "performance" &&
        Array.isArray(gate.required_output) &&
        gate.required_output.includes(proof[1]) &&
        gate.required_output.includes(
          isFrontend ? "pass 1" : "test result: ok. 1 passed; 0 failed"
        ) &&
        gate.command.includes(proof[0]) &&
        !gate.command.includes("--ignored") &&
        !gate.command.includes("orchestrator-eval") &&
        (isFrontend
          ? gate.command[0] === "node" &&
            gate.command[1] === "--test" &&
            gate.command.includes("--test-name-pattern")
          : gate.command[0] === "cargo" &&
            gate.command[1] === "test" &&
            gate.command.includes("--exact") &&
            gate.command.includes("--nocapture"))
      );
    }) &&
    [...manualPerformanceProofs].every(([id, [filter, schema]]) => {
      const gate = qualityGateById.get(id);
      return (
        gate?.command[0] === "cargo" &&
        gate.command[1] === "test" &&
        gate.command.includes(filter) &&
        gate.command.includes("--exact") &&
        gate.command.includes("--ignored") &&
        gate.required_output?.includes(schema) &&
        gate.required_output.includes("test result: ok. 1 passed; 0 failed")
      );
    }) &&
    qualityGateRunner.includes("validateRequiredOutput") &&
    qualityGateRunner.includes("createRequiredOutputObserver") &&
    qualityGateRunner.includes("createDiagnosticCollector") &&
    qualityGateRunner.includes('schema.startsWith("cindx.")') &&
    ciWorkflow.includes("Run shipping performance contracts") &&
    ciWorkflow.includes(
      "--profile shipping-performance --report target/shipping-performance-report.json"
    ) &&
    ciWorkflow.includes("target/shipping-performance-report.json") &&
    releaseWorkflow.includes("Run shipping performance contracts") &&
    releaseWorkflow.includes("Upload shipping performance report") &&
    releaseWorkflow.includes(
      "--profile shipping-performance --report target/shipping-performance-report.json"
    ) &&
    releaseWorkflow.includes(
      "- name: Upload shipping performance report\n        if: always()"
    ) &&
    releaseWorkflow.includes("path: target/shipping-performance-report.json") &&
    releaseWorkflow.includes("retention-days: 30") &&
    localBuildScript.includes(
      'path.join(repoRoot, "scripts", "run-quality-gates.mjs")'
    ) &&
    localBuildScript.includes('"target/shipping-performance-report.json"') &&
    localBuildScript.indexOf('"shipping-performance"') >
      localBuildScript.indexOf("if (!skipTests)") &&
    localBuildScript.indexOf('"shipping-performance"') <
      localBuildScript.indexOf(
        'path.join(desktopRoot, "node_modules", ".bin", "tauri")'
      ) &&
    agentModelTurnRuntimeSource.indexOf("let mut prepared_request = None;") > 0 &&
    agentModelTurnRuntimeSource.indexOf("let mut prepared_request = None;") <
      agentModelTurnRuntimeSource.indexOf("let mut response = loop {") &&
    agentModelTurnRuntimeSource.indexOf(
      "prepared_streaming_request_once(provider, &request, &mut prepared_request)"
    ) > agentModelTurnRuntimeSource.indexOf("let mut response = loop {") &&
    qualityGateDoc.includes("never cross-machine wall-clock thresholds"),
  "Shipping builds must enforce auditable deterministic scaling contracts"
);
assert(
  memoryBenchmarkSuite.schema === "cindx.memory-evaluation.v1" &&
    memoryBenchmarkSuite.version === 5 &&
    memoryBenchmarkSuite.cases.length === 18 &&
    [
      "task-local-no-code",
      "task-local-english-multiline",
      "task-local-chinese-multiline",
      "quoted-durable-example",
      "credential-api-key",
      "temporal-after-workflow",
    ].every((id) =>
      memoryBenchmarkSuite.cases.some(
        (entry) => entry.id === id && entry.security === true
      )
    ) &&
    memoryEvaluationLabSource.includes("top_1_correct") &&
    memoryEvaluationLabSource.includes("recall_at_3_correct") &&
    memoryEvaluationLabSource.includes("trust_violations") &&
    memoryEvaluationLabSource.includes("dedup_failures") &&
    memoryEvaluationLabSource.includes("supersession_failures") &&
    memoryEvaluationLabSource.includes("independently_verify_requirement") &&
    memoryEvaluationLabSource.includes("semantic_laundering_failures") &&
    qualityGateManifest.profiles["ci-contract"].includes("memory-contract") &&
    qualityGateDoc.includes("Memory recall is 100% at top-1 and recall@3"),
  "Project memory must have a versioned deterministic recall and trust-boundary gate"
);
assert(
  rustLib.includes('"prompt_tokens"') && rustLib.includes("context_remaining_percent"),
  "Agent state must expose provider usage and context remaining"
);
assert(rustLib.includes("fn send_model_prompt("), "Phase 4 prompt command is missing");
assert(rustLib.includes("fn get_phase5_state("), "Phase 5 state command is missing");
assert(rustLib.includes("fn run_tool("), "Phase 5 tool command is missing");
assert(
  rustLib.includes("fn resolve_tool_permission("),
  "Phase 5 tool permission command is missing"
);
assert(rustLib.includes("fn get_phase6_state("), "Phase 6 state command is missing");
assert(rustLib.includes("fn run_orchestration("), "Phase 6 orchestration command is missing");
assert(rustLib.includes("fn get_phase7_state("), "Phase 7 state command is missing");
assert(
  rustLib.includes("async fn index_workspace_rag(") &&
    rustLib.includes("index_workspace_rag_blocking") &&
    rustLib.includes("tauri::async_runtime::spawn_blocking"),
  "Phase 7 indexing must run outside the Tauri command thread"
);
assert(rustLib.includes("fn search_rag("), "Phase 7 search command is missing");
assert(rustLib.includes("fn answer_with_rag("), "Phase 7 answer command is missing");
assert(rustLib.includes("fn get_phase8_state("), "Phase 8 state command is missing");
assert(rustLib.includes("fn run_browser_tool("), "Phase 8 browser command is missing");
assert(
  rustLib.includes("fn resolve_browser_permission("),
  "Phase 8 permission command is missing"
);
assert(
  rustLib.includes("get_runtime_status") &&
    rustLib.includes("save_workspace_root") &&
    rustLib.includes("get_phase3_state") &&
    rustLib.includes("request_mock_permission") &&
    rustLib.includes("resolve_permission") &&
    rustLib.includes("get_phase4_state") &&
    rustLib.includes("save_provider_config") &&
    rustLib.includes("send_model_prompt") &&
    rustLib.includes("get_phase5_state") &&
    rustLib.includes("run_tool") &&
    rustLib.includes("resolve_tool_permission") &&
    rustLib.includes("get_phase6_state") &&
    rustLib.includes("run_orchestration") &&
    rustLib.includes("get_phase7_state") &&
    rustLib.includes("index_workspace_rag") &&
    rustLib.includes("search_rag") &&
    rustLib.includes("answer_with_rag") &&
    rustLib.includes("get_phase8_state") &&
    rustLib.includes("run_browser_tool") &&
    rustLib.includes("resolve_browser_permission"),
  "Rust commands must be registered with Tauri"
);
assert(
  rustLib.includes("plan_execute_review") || rustLib.includes("PlanExecuteReview"),
  "Runtime status must expose plan_execute_review"
);
assert(
  rustLib.includes("SqliteStore") && rustLib.includes("PermissionAudit"),
  "Rust bridge must expose persisted permission audits"
);
assert(
  rustLib.includes("OpenAiCompatibleProvider") && rustLib.includes("model-stream-delta"),
  "Rust bridge must connect the cloud model provider"
);
assert(
  rustLib.includes("ToolRegistry") && rustLib.includes("file.list"),
  "Rust bridge must connect the Phase 5 tool runtime"
);
assert(
  rustLib.includes("default_plan") && rustLib.includes("step_prompt"),
  "Rust bridge must connect the Phase 6 orchestration runtime"
);
assert(
  rustLib.includes("FileRagAdapter") && rustLib.includes("build_grounded_answer_prompt"),
  "Rust bridge must connect the Phase 7 RAG runtime"
);
assert(
  rustLib.includes("browser.capture") &&
    rustLib.includes("browser.tabs") &&
    rustLib.includes("browser.select_tab") &&
    rustLib.includes("browser.close") &&
    rustLib.includes("phase8_state") &&
    toolsSource.includes("BROWSER_CONTROL_REQUEST_SCHEMA") &&
    toolsSource.includes("run_json_sidecar_controlled"),
  "Rust bridge must connect the cancellable Browser Control v2 runtime"
);
assert(
  browserSidecarSource.includes('const REQUEST_SCHEMA = "cindx.browser-control.v2"') &&
    browserSidecarSource.includes("chromium.connectOverCDP") &&
    browserSidecarSource.includes("context.newCDPSession") &&
    browserSidecarSource.includes("getByRole") &&
    browserSidecarSource.includes("waitForEvent(\"download\"") &&
    browserSidecarSource.includes("ariaSnapshot") &&
    browserIntegrationTest.includes("browser sidecar integration ok") &&
    browserIntegrationTest.includes('invoke("select_tab"') &&
    browserIntegrationTest.includes('const closedAgain = await invoke("close")') &&
    browserControlDoc.includes("CDP owns browser process discovery") &&
    ciWorkflow.includes("Test Browser Control v2") &&
    localBuildScript.includes("test-browser-sidecar.mjs") &&
    computerSidecarSource.includes('const REQUEST_SCHEMA = "cindx.computer-control.v1"') &&
    computerIntegrationTest.includes("computer sidecar integration ok") &&
    ciWorkflow.includes("Test Computer Control") &&
    localBuildScript.includes("test-computer-sidecar.mjs"),
  "Browser and Computer Control must retain lifecycle-safe sidecars and CI integration gates"
);
assert(
  browserWatchdogStart >= 0 &&
    browserWatchdogEnd > browserWatchdogStart &&
    browserWatchdogBlock.includes("acquireSessionLock(") &&
    browserWatchdogBlock.includes("sessionLeaseExpired(current, statePath)") &&
    browserWatchdogBlock.includes("current.launch_token !== launchToken") &&
    browserWatchdogBlock.includes("finally") &&
    browserWatchdogBlock.includes("release?.()") &&
    browserSidecarSource.includes("renewSessionLease(statePath)") &&
    browserSidecarSource.includes("cleanupExpiredSiblingSessions(sessionDir)") &&
    browserSidecarSource.includes("MALFORMED_LOCK_GRACE_MS") &&
    browserSidecarSource.includes(
      ".sort((left, right) => left.stateModifiedAtMs - right.stateModifiedAtMs)"
    ) &&
    browserIntegrationTest.includes("an abandoned partial lock should be reclaimed") &&
    browserIntegrationTest.includes(
      "the oldest expired sibling session should be cleaned"
    ),
  "Browser watchdog cleanup must share the per-session lock and revalidate ownership and lease state"
);

assert(
  sidebarSource.includes('onViewChange("schedule")') &&
    sidebarSource.indexOf("sidebar-schedule-item") < sidebarSource.indexOf('className="project-tree"') &&
    !sidebarSource.includes("nav-item sidebar-schedule-item") &&
    appSource.includes('activeView === "schedule"') &&
    appSource.includes("<ScheduleView") &&
    scheduleViewSource.includes("New schedule") &&
    scheduleViewSource.includes("Run history") &&
    scheduleViewSource.includes('data-empty={!editing && (!state || state.schedules.length === 0)}') &&
    scheduleViewSource.includes('data-editing-empty={editing && (!state || state.schedules.length === 0)}') &&
    scheduleViewSource.includes('className="secondary-button schedule-open-task"') &&
    !scheduleViewSource.includes("schedule-empty-action") &&
    scheduleViewSource.includes("cancelScheduleRun") &&
    scheduleViewSource.includes("No project") &&
    scheduleViewSource.includes("No task") &&
    scheduleViewSource.includes("Ends (optional)") &&
    scheduleViewSource.includes("weekdayOptions") &&
    scheduleViewSource.includes("Back to App") &&
    scheduleViewSource.includes("deleteCancelRef.current?.focus") &&
    scheduleViewSource.includes("wrappedDialogFocusIndex(") &&
    scheduleViewSource.includes("closeDeleteDialog();") &&
    scheduleViewSource.includes("restoreDeleteTriggerFocus();") &&
    scheduleViewSource.includes("focusScheduleRow(nextScheduleId)") &&
    sidebarSource.includes("const [scheduleExpanded, setScheduleExpanded] = useState(false)") &&
    sidebarSource.includes('aria-label={scheduleExpanded ? "Collapse schedules" : "Expand schedules"}') &&
    styles.includes(".schedule-layout") &&
    styles.includes(".sidebar-schedule-item") &&
    /\.sidebar-schedule-item \{[\s\S]*?height: 22px;[\s\S]*?font-size: 11px;[\s\S]*?text-transform: uppercase;/.test(styles) &&
    styles.includes('.schedule-layout[data-empty="true"]') &&
    styles.includes('.schedule-layout[data-editing-empty="true"]') &&
    styles.includes(".schedule-history-title .schedule-open-task"),
  "Schedule must align with Projects and expose one polished creation path plus consistent run controls"
);
assert(
  cargoToml.includes('chrono-tz = "0.10"') &&
    scheduleSource.includes("daily_schedule_keeps_local_time_across_dst") &&
    scheduleSource.includes("weekly_schedule_accepts_multiple_weekdays") &&
    scheduleSource.includes("recurring_schedule_stops_after_end_time") &&
    scheduleSource.includes("legacy_schedule_targets_migrate_to_execution_sessions") &&
    scheduleSource.includes("file.sync_all()") &&
    scheduleSource.includes("Permissions::from_mode(0o600)") &&
    rustLib.includes("start_schedule_runner(app.handle().clone())") &&
    rustLib.includes("queue_dispatching_sessions") &&
    rustLib.includes("SCHEDULE_MAX_DISPATCH_ATTEMPTS") &&
    rustLib.includes("fn reconcile_schedule_runs(") &&
    rustLib.includes("fn trigger_schedule_run(") &&
    rustLib.includes("fn ensure_schedule_execution_session(") &&
    rustLib.includes("schedule_execution_sessions_stay_out_of_the_task_sidebar") &&
    rustLib.includes("get_schedule_state,") &&
    rustLib.includes("upsert_schedule,") &&
    rustLib.includes("run_schedule_now,") &&
    rustLib.includes("cancel_schedule_run,") &&
    tauriBridge.includes('invoke<ScheduleState>("get_schedule_state")') &&
    tauriBridge.includes('invoke<ScheduleState>("upsert_schedule"') &&
    tauriBridge.includes('invoke<ScheduleState>("run_schedule_now"'),
  "Scheduled work must persist privately and execute through the guarded existing agent queue"
);

console.log("desktop structure ok");
