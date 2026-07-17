import fs from "node:fs";
import path from "node:path";

const root = process.cwd();

const read = (relativePath) =>
  fs.readFileSync(path.join(root, relativePath), "utf8");

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
const sessionThreadSource = read("apps/desktop/src/components/SessionThread.tsx");
const composerSource = read("apps/desktop/src/components/Composer.tsx");
const inspectorSource = read("apps/desktop/src/components/Inspector.tsx");
const sidebarSource = read("apps/desktop/src/components/Sidebar.tsx");
const disclosureTriangleSource = read(
  "apps/desktop/src/components/DisclosureTriangle.tsx"
);
const knowledgeGraphSource = read(
  "apps/desktop/src/components/KnowledgeGraph.tsx"
);
const traceStatusIconSource = read("apps/desktop/src/components/TraceStatusIcon.tsx");
const styles = read("apps/desktop/src/styles.css");
const tauriBridge = read("apps/desktop/src/tauri.ts");
const localBuildScript = read("scripts/build-local-app.mjs");
const browserSidecarSource = read("scripts/sidecars/browser-sidecar.js");
const browserIntegrationTest = read("scripts/test-browser-sidecar.mjs");
const rustLib = read("apps/desktop/src-tauri/src/lib.rs");
const runControlSource = read("apps/desktop/src-tauri/src/run_control.rs");
const cargoToml = read("apps/desktop/src-tauri/Cargo.toml");
const cargoLock = read("apps/desktop/src-tauri/Cargo.lock");
const runTauriSource = read("scripts/run-tauri.mjs");
const stampBuildVersionSource = read("scripts/stamp-build-version.mjs");
const releaseWorkflow = read(".github/workflows/release.yml");
const unsignedReleaseStart = releaseWorkflow.indexOf(
  "- name: Build and publish unsigned Universal app"
);
const unsignedReleaseBlock =
  unsignedReleaseStart >= 0 ? releaseWorkflow.slice(unsignedReleaseStart) : "";
const ciWorkflow = read(".github/workflows/ci.yml");
const releaseVersionCheck = read("scripts/check-release-version.mjs");
const toolsSource = read("crates/tools/src/lib.rs");
const agentStorageSource = read("crates/agent-storage/src/lib.rs");
const agentSkillsSource = read("crates/agent-skills/src/lib.rs");
const builtinSkillCreator = read("crates/agent-skills/builtins/skill-creator/SKILL.md");
const modelProviderSource = read("crates/model-provider/src/lib.rs");
const modelProviderCargo = read("crates/model-provider/Cargo.toml");
const ragSource = read("crates/agent-rag/src/lib.rs");
const graphSource = read("crates/agent-graph/src/lib.rs");
const agentMemorySource = read("crates/agent-memory/src/lib.rs");
const agentRuntimeSource = read("crates/agent-runtime/src/lib.rs");
const coreAgentPrompt = read("crates/agent-runtime/src/core_prompt.txt");
const orchestratorSource = read("crates/orchestrator/src/lib.rs");
const promptEvolutionSource = read("crates/orchestrator/src/prompt_evolution.rs");
const benchmarkSource = read("crates/orchestrator/src/benchmark.rs");
const benchmarkSuite = JSON.parse(read("benchmarks/agent/core-v1.json"));
const benchmarkBaseline = JSON.parse(read("benchmarks/agent/core-v1-baseline.json"));
const evaluationLabSource = read("crates/orchestrator/examples/evaluation_lab.rs");
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

assert(packageJson.name === "cindx-desktop", "desktop package name changed");
assert(packageJson.scripts.dev.includes("vite"), "desktop dev script must run Vite");
assert(packageJson.scripts.build.includes("vite build"), "desktop build script must build Vite");
assert(
  packageJson.scripts.tauri.includes("run-tauri.mjs") &&
    runTauriSource.includes('args[0] === "build"') &&
    runTauriSource.includes("nextPatchVersion") &&
    runTauriSource.includes("rollbackDesktopVersion") &&
    runTauriSource.includes("stable-aarch64-apple-darwin"),
  "desktop Tauri builds must increment the patch version and retain the Rust toolchain PATH"
);
assert(
  releaseWorkflow.includes("tauriScript: ./node_modules/.bin/tauri"),
  "release workflow must bypass the local auto-versioning wrapper"
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
    stampBuildVersionSource.includes("Math.max(Number(match[3]) + 1, runNumber)") &&
    stampBuildVersionSource.includes("apps/desktop/src-tauri/Cargo.lock"),
  "Every CI app build must stamp one synchronized monotonic patch version"
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
assert(
  tauriConfig.app.windows.every((window) => window.visible === false) &&
    !rustLib.includes(".on_page_load(|webview, payload|") &&
    rustLib.includes("fn reveal_main_window(app: tauri::AppHandle)") &&
    rustLib.includes("schedule_main_window_reveal_fallback") &&
    rustLib.includes("MAIN_WINDOW_REVEAL_FALLBACK_MS: u64 = 12_000") &&
    rustLib.includes("fn repair_macos_traffic_light_position(") &&
    rustLib.includes("fn schedule_macos_traffic_light_position_repair(") &&
    rustLib.includes("MACOS_TRAFFIC_LIGHT_REPAIR_GENERATION") &&
    rustLib.includes("MACOS_TRAFFIC_LIGHT_REPAIR_DELAY_MS: u64 = 48") &&
    rustLib.includes("repair_macos_traffic_light_position(&window)?;") &&
    rustLib.includes("let _ = repair_macos_traffic_light_position(&window);") &&
    rustLib.includes("tauri::WindowEvent::Resized(_)") &&
    rustLib.includes("tauri::WindowEvent::Moved(_)") &&
    rustLib.includes("tauri::WindowEvent::Focused(true)") &&
    rustLib.includes("tauri::WindowEvent::ScaleFactorChanged { .. }") &&
    rustLib.includes("tauri::WindowEvent::ThemeChanged(_)") &&
    tauriBridge.includes('invoke<void>("reveal_main_window")') &&
    appSource.includes("startupWindowRevealRequestedRef") &&
    appSource.includes("await document.fonts.ready") &&
    appSource.includes("await revealMainWindow()") &&
    appSource.includes("!agentStateCacheRef.current.has(state.activeSessionId)") &&
    !appSource.includes("agentState?.sessionId !== projectSessionState.activeSessionId") &&
    !appSource.includes("revealAfterStableFrame"),
  "The native window must reveal a stable loading frame without waiting for large session history and repair native controls after AppKit relayouts"
);
const titlebarHeight = 46;
// This is the user-confirmed macOS alignment; do not retune it indirectly.
const confirmedMacOSTrafficLightY = 25;

assert(
  tauriConfig.app.windows.every(
    (window) => window.trafficLightPosition?.y === confirmedMacOSTrafficLightY
  ) &&
    styles.includes(`--titlebar-height: ${titlebarHeight}px`) &&
    styles.includes("--titlebar-control-size: 28px") &&
    styles.includes("grid-template-rows: var(--titlebar-height) minmax(0, 1fr)") &&
    styles.includes(
      "top: calc((var(--titlebar-height) - var(--titlebar-control-size)) / 2)"
    ) &&
    !styles.includes("--titlebar-content-offset-y"),
  "Native traffic lights must retain the user-confirmed y=25 alignment"
);
assert(
  tauriConfig.bundle.icon.includes("icons/icon.icns"),
  "Tauri bundle must use the generated macOS app icon"
);
assert(
  localBuildScript.includes("local-build-number") &&
    localBuildScript.includes('args.has("--ephemeral-target")') &&
    localBuildScript.includes("CARGO_TARGET_DIR: targetRoot") &&
    localBuildScript.includes('path.join(os.homedir(), ".cargo", "bin")') &&
    localBuildScript.includes('CINDX_STARTUP_PROBE: "1"') &&
    localBuildScript.includes('"--identifier"') &&
    localBuildScript.includes('const installApp = !args.has("--no-install")') &&
    localBuildScript.includes('run("pkill", ["-x", "cindx-desktop"]') &&
    localBuildScript.includes('waitForProcessExit("cindx-desktop")') &&
    localBuildScript.includes("restoreVersions()") &&
    packageJson.scripts?.["build:app"] === "node ../../scripts/build-local-app.mjs",
  "Local builds must auto-version, probe, sign, install, package, and restore source versions"
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
    sessionThreadSource.includes('from "markdown-to-jsx"') &&
    sessionThreadSource.includes("disableParsingRawHTML: true") &&
    sessionThreadSource.includes("content={item.message.content}") &&
    sessionThreadSource.includes("content={streamAnswer}") &&
    sessionThreadSource.includes("streaming") &&
    sessionThreadSource.includes("function MarkdownCodeBlock") &&
    sessionThreadSource.includes("component: MarkdownCodeBlock") &&
    sessionThreadSource.includes('aria-label="Copy code"') &&
    sessionThreadSource.includes('role={!isUser && !isAssistant ? "button" : undefined}') &&
    sessionThreadSource.includes('showClipboardToast("Copied to clipboard")') &&
    sessionThreadSource.includes("navigator.clipboard.writeText(content)") &&
    styles.includes(".thread-markdown pre code") &&
    styles.includes(".thread-code-block-header") &&
    styles.includes(".clipboard-toast") &&
    styles.includes(".thread-markdown table"),
  "Assistant messages must render safe Markdown with copyable code blocks and clipboard feedback"
);
assert(
  sessionThreadSource.includes("useLayoutEffect") &&
    sessionThreadSource.includes("knownMessageIdsRef") &&
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
    modelProviderCargo.includes('reqwest = { version = "0.13.4", features = ["stream"] }') &&
    !modelProviderSource.includes('Command::new("/usr/bin/curl")') &&
    modelProviderSource.includes("streamed_tool_calls") &&
    modelProviderSource.includes("MODEL_REQUEST_CANCELLED") &&
    rustLib.includes("agent_run_controls") &&
    rustLib.includes("agent_run_should_stop") &&
    runControlSource.includes("struct AgentRunControl") &&
    runControlSource.includes("DeadlineExceeded") &&
    rustLib.includes("request_agent_run_cancel") &&
    rustLib.includes("emit_agent_stream_delta") &&
    appSource.includes("payload.sessionId !== activeSessionIdRef.current") &&
    appSource.includes("if (payload.reset)") &&
    appSource.includes("streamBuffer += payload.delta") &&
    appSource.includes("window.setTimeout(flushStreamBuffer, 80)") &&
    appSource.includes("markSessionBusy(sessionId, false)") &&
    tauriBridge.includes("sessionId: string | null") &&
    tauriBridge.includes("reset: boolean"),
  "Agent output must stream by session and Stop must cancel the active provider request"
);
assert(
  rustLib.includes("should_run_agent_knowledge_retrieval(&routing_context)") &&
    orchestratorSource.includes("is_capability_question") &&
    orchestratorSource.includes("is_lightweight_direct") &&
    orchestratorSource.includes("learned_router_cannot_upgrade_a_lightweight_coding_question") &&
    rustLib.includes("index_graph_chunks_cancellable") &&
    rustLib.includes("upsert_all(extractions)") &&
    ragSource.includes("index_workspace_cancellable") &&
    ragSource.includes("RAG_INDEX_CANCELLED") &&
    modelProviderSource.includes("embed_cancellable") &&
    graphSource.includes("pub fn upsert_all"),
  "Lightweight turns must skip retrieval and knowledge preparation must remain cancellable"
);
assert(
  appSource.includes("sessionSelectionRequestRef") &&
    appSource.includes("sessionSelectionQueueRef") &&
    appSource.includes("sessionRefreshRequestRef") &&
    appSource.includes("enqueueProjectSessionSelection") &&
    appSource.includes("if (sessionId === activeSessionIdRef.current) return") &&
    appSource.includes("getContextState(sessionId)") &&
    appSource.includes("if (workspaceChanged) refreshWorkspaceScopedState()") &&
    !sessionRefreshBlock.includes("setPhase7(await getPhase7State())") &&
    tauriBridge.includes('invoke<ContextState>("get_context_state", {') &&
    rustLib.includes("session_id: Option<String>") &&
    rustLib.includes("project_session_metadata_for_session(&state, session_id.as_deref())"),
  "Session switching must prioritize chat state and defer workspace-wide refreshes"
);
assert(
  appSource.includes("SESSION_STATE_CACHE_LIMIT = 12") &&
    appSource.includes("agentStateCacheRef") &&
    appSource.includes("agentTraceCacheRef") &&
    appSource.includes("contextStateCacheRef") &&
    appSource.includes("requestSessionAgentState(sessionId)") &&
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
    agentStorageSource.includes("event_scope_columns_v1") &&
    agentStorageSource.includes("list_by_task_and_metadata_after") &&
    agentStorageSource.includes("save_read_model") &&
    rustLib.includes("AGENT_SESSION_READ_MODEL_NAMESPACE") &&
    rustLib.includes("struct AgentSessionReadModel") &&
    rustLib.includes("struct AgentStateDelta") &&
    tauriBridge.includes("export async function getAgentStateDelta") &&
    appSource.includes("function mergeAgentStateDelta") &&
    appSource.includes("getAgentStateDelta(sessionId"),
  "Active session polling must use indexed event deltas and a persistent read model"
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
    appSource.includes("function mergeAgentStateSnapshot") &&
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
    sessionThreadSource.includes('loading ? "Loading conversation" : "No messages yet"'),
  "Cold session loads must show an explicit loading state instead of a blank thread"
);
assert(
  sessionThreadSource.includes("export const SessionThread = memo(function SessionThread") &&
    sessionThreadSource.includes("const ToolChainDisclosure = memo(function ToolChainDisclosure") &&
    sessionThreadSource.includes("{open && (") &&
    sessionThreadSource.includes("threadContentRef") &&
    sessionThreadSource.includes("resizeObserver.observe(threadContentRef.current)") &&
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
    sessionThreadSource.includes('className="thread-jump-latest"') &&
    sessionThreadSource.includes('aria-label="Jump to latest output"') &&
    styles.includes("backdrop-filter: saturate(150%) blur(18px)") &&
    styles.includes("@keyframes thread-jump-latest-in"),
  "Scrolling away from the latest output must reveal the frosted jump-to-latest control"
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
  appSource.includes('aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}') &&
    !inspectorSource.includes("Close inspector"),
  "Inspector must use one stable toggle instead of duplicate controls"
);
assert(
  appSource.includes('const [inspectorOpen, setInspectorOpen] = useState(false)') &&
    appSource.includes('const [inspectorOpenBeforeSettings, setInspectorOpenBeforeSettings] = useState(false)') &&
    appSource.includes("onOutputCreated={() =>") &&
    inspectorSource.includes("outputSignaturesBySessionRef") &&
    inspectorSource.includes("onOutputCreated();"),
  "Inspector must start closed and open when the active session creates an output"
);
assert(
  appSource.includes('DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible"') &&
    appSource.includes("loadDebugAlwaysVisible") &&
    appSource.includes("Always show Debug") &&
    appSource.includes("showDebug={debugAlwaysVisible}") &&
    inspectorSource.includes('hidden={!showDebug}') &&
    inspectorSource.includes("showDebug: boolean"),
  "Debug entry must stay hidden by default and use the persisted Settings preference"
);
assert(
  appSource.includes("window-toolbar") &&
    appSource.includes("data-sidebar-open={sidebarOpen}") &&
    appSource.includes('aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}'),
  "The titlebar must expose a stable sidebar toggle"
);
assert(
  appSource.includes('className="window-toolbar" data-tauri-drag-region') &&
    styles.includes(".window-workspace-header") &&
    styles.includes("pointer-events: none") &&
    !appSource.includes("getCurrentWindow().startDragging()"),
  "The custom titlebar must expose a native drag region without blocking pane controls"
);
assert(
  appSource.includes("workspaceViewBeforeSettings") &&
    appSource.includes('if (activeView === "settings")') &&
    appSource.includes("showWorkspaceView(workspaceViewBeforeSettings)"),
  "Settings must toggle back to the previous workspace view"
);
assert(
  /function handleSelectSession\(sessionId: string\) \{\s*showTimelineView\(\);\s*if \(sessionId === activeSessionIdRef\.current\) return;/.test(
    appSource
  ),
  "Selecting any sidebar session must leave Settings, including the active session"
);
assert(
  appSource.includes("PanelLeftClose") &&
    appSource.includes("PanelLeftOpen") &&
    appSource.includes("PanelRightClose") &&
    appSource.includes("PanelRightOpen") &&
    styles.includes("window-pane-toggle-icon"),
  "Pane controls must animate between explicit open and close icons"
);
assert(
  styles.includes("transition: grid-template-columns 180ms") &&
    styles.includes("prefers-reduced-motion: reduce"),
  "Pane transitions must be subtle and respect reduced-motion preferences"
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
  appSource.includes("window-toolbar-panel-left") &&
    appSource.includes("window-toolbar-panel-right") &&
    appSource.includes("data-active-view={activeView}") &&
    appSource.includes("data-view={activeView}") &&
    styles.includes("background: rgba(255, 255, 255, 0.96)") &&
    styles.includes('.workspace[data-view="timeline"]') &&
    styles.includes("backdrop-filter: saturate(145%) blur(18px)") &&
    /\.window-toolbar-panel \{[\s\S]*?background: var\(--panel\);/.test(styles) &&
    tauriConfig.app.macOSPrivateApi === true &&
    tauriConfig.app.windows.every((window) => window.transparent === true) &&
    cargoToml.includes('features = ["macos-private-api"]') &&
    !rustLib.includes("window_vibrancy::apply_vibrancy"),
  "Timeline content must scroll beneath the translucent titlebar glass surface"
);
assert(
  /\.app-shell\[data-active-view="settings"\] \{[\s\S]*?transition: none;/.test(styles) &&
    /\.app-shell\[data-active-view="settings"\] \.window-toolbar \{[\s\S]*?background: var\(--bg\);/.test(
      styles
    ) &&
    /\.settings-view \{[\s\S]*?overflow-y: auto;[\s\S]*?scrollbar-gutter: stable;/.test(
      styles
    ) &&
    styles.includes("--scrollbar-size: 6px") &&
    styles.includes("*::-webkit-scrollbar-thumb"),
  "Settings must scroll only when needed and use the shared compact scrollbar"
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
  composerSource.includes('if (working || canStop) return;') &&
    !composerSource.includes('disabled={working || canStop}\n              aria-keyshortcuts="Enter"'),
  "Composer must remain editable while the agent runs and require an explicit stop before sending"
);
assert(
  composerSource.includes("onCompositionStart") &&
    composerSource.includes("onCompositionEnd") &&
    composerSource.includes("compositionJustEndedRef") &&
    composerSource.includes("nativeEvent.keyCode === 229"),
  "Composer must not submit macOS IME candidate-selection keystrokes"
);
assert(
  composerSource.includes("onPaste={(event) =>") &&
    composerSource.includes("event.clipboardData.items") &&
    composerSource.includes('item.type.startsWith("image/")') &&
    composerSource.includes("onPickAttachments(pastedImages)") &&
    composerSource.includes("function ComposerAttachmentPreview") &&
    composerSource.includes("readArtifactPreview(attachment.path)") &&
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
  tauriBridge.includes("attachments?: AgentAttachment[]") &&
    appSource.includes("attachments\n    };") &&
    rustLib.includes('"attachment_mime_types".to_string()') &&
    rustLib.includes("attachments: attachment_views_from_event(event)") &&
    sessionThreadSource.includes("function UserMessageAttachments") &&
    sessionThreadSource.includes('className="thread-message-attachments"') &&
    sessionThreadSource.includes("readArtifactPreview(attachment.path)") &&
    styles.includes('.thread-message-attachment[data-image="true"]'),
  "Sent attachments must persist with user messages and render as image or file bubbles"
);
assert(
  sessionThreadSource.includes("thread.scrollTop = thread.scrollHeight") &&
    sessionThreadSource.includes("useLayoutEffect(() => {") &&
    sessionThreadSource.includes('message.sequence ?? `${message.role}-${index}`') &&
    appSource.includes("optimisticUserMessagesRef") &&
    appSource.includes("messagesWithOptimisticUserMessage") &&
    appSource.includes("messages={visibleAgentMessages}"),
  "Session thread must show submitted user messages before paint and preserve them during polling"
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
    sessionThreadSource.includes("minimapMarkerPosition(index, minimapMarkers.length)") &&
    sessionThreadSource.includes("MIN_MINIMAP_MARKERS = 2") &&
    sessionThreadSource.includes("data-edge-fade") &&
    sessionThreadSource.includes("data-wave-distance") &&
    sessionThreadSource.includes("thread-minimap-preview") &&
    sessionThreadSource.includes("}, 420)"),
  "Minimap markers must grow evenly from the vertical center with a five-tick hover wave and delayed preview"
);
assert(
  sessionThreadSource.includes("MAX_MINIMAP_MARKERS = 32") &&
    sessionThreadSource.includes('role !== "user"') &&
    sessionThreadSource.includes('role !== "assistant"') &&
    sessionThreadSource.includes('preview.toLowerCase() === "tool request"') &&
    sessionThreadSource.includes("rowIndexByItemId.get(marker.id)"),
  "Minimap must index only sparse, substantive user and model output anchors"
);
assert(
  packageJson.dependencies["@tanstack/react-virtual"] &&
    sessionThreadSource.includes('import { useVirtualizer } from "@tanstack/react-virtual"') &&
    sessionThreadSource.includes("const rowVirtualizer = useVirtualizer") &&
    sessionThreadSource.includes("const virtualRows = rowVirtualizer.getVirtualItems()") &&
    sessionThreadSource.includes("ref={rowVirtualizer.measureElement}") &&
    sessionThreadSource.includes('className="thread-virtual-list"') &&
    sessionThreadSource.includes('className="thread-virtual-row"') &&
    styles.includes(".thread-virtual-list") &&
    styles.includes(".thread-virtual-row"),
  "Long sessions must virtualize variable-height rows instead of mounting the full transcript"
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
  sessionThreadSource.includes('data-content-ready={contentReady}') &&
    sessionThreadSource.includes('className="session-thread-empty-state"') &&
    sessionThreadSource.includes("rowVirtualizer.measure()") &&
    sessionThreadSource.includes("setContentReady(true)") &&
    appSource.includes("const sessionPrefetchKey = useMemo") &&
    appSource.includes("requestSessionAgentState(sessionId).catch(() => null)") &&
    /\.session-thread\[data-content-ready="false"\] \.thread-content \{[\s\S]*?opacity: 0;/.test(
      styles
    ),
  "Initial session hydration must prewarm local state and reveal measured rows without layout jumps"
);
assert(
  sessionThreadSource.includes("thread-message-actions") &&
    sessionThreadSource.includes("onEditMessage"),
  "User messages must expose copy and edit actions"
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
  sessionThreadSource.includes("groupThreadItems") &&
    sessionThreadSource.includes('!content || content === "tool request"') &&
    !sessionThreadSource.includes("containsToolActivity") &&
    sessionThreadSource.includes("while (end < items.length && isActivityCandidate(items[end]))") &&
    sessionThreadSource.includes("thread-tool-chain") &&
    sessionThreadSource.includes("Agent actions") &&
    sessionThreadSource.includes('className="thread-tool-chain-chevron"') &&
    !sessionThreadSource.includes("toolChainStatus(row.items)") &&
    sessionThreadSource.includes("<ToolChainItem") &&
    styles.includes(".thread-tool-chain-items") &&
    styles.includes(".thread-tool-chain[open] > summary .thread-tool-chain-chevron"),
  "All contiguous agent reasoning, collaboration, and tool activity must default to one parent disclosure"
);
assert(
  disclosureTriangleSource.includes('viewBox="0 0 16 16"') &&
    disclosureTriangleSource.includes("c.52 0 1 .28 1.26.73") &&
    !disclosureTriangleSource.includes('import { Triangle }') &&
    (sessionThreadSource.match(/<DisclosureTriangle/g)?.length ?? 0) >= 2 &&
    !inspectorSource.includes("DisclosureTriangle") &&
    !appSource.includes("DisclosureTriangle") &&
    (appSource.match(/className="settings-disclosure-chevron"/g)?.length ?? 0) === 8 &&
    (appSource.match(/className="settings-action-chevron"/g)?.length ?? 0) === 1 &&
    (inspectorSource.match(/<ChevronRight/g)?.length ?? 0) >= 1 &&
    inspectorSource.includes('className="inspector-debug-chevron"') &&
    !styles.includes("advanced-settings summary::before") &&
    styles.includes("details[open] > summary .disclosure-triangle") &&
    styles.includes("details[open] > summary .settings-disclosure-chevron") &&
    styles.includes(".settings-action-chevron") &&
    styles.includes("vertical-align: middle") &&
    styles.includes("transform-box: fill-box") &&
    styles.includes("transition: transform 180ms") &&
    styles.includes(".secondary-button:hover:not(:disabled) .settings-action-chevron"),
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
  !sessionThreadSource.includes("threadTimeFormatter") &&
    !sessionThreadSource.includes("formatThreadTime") &&
    !sessionThreadSource.includes("<time") &&
    !styles.includes(".thread-message-actions time") &&
    !styles.includes(".thread-message-agent-meta"),
  "Session messages and activity must keep timestamps exclusively in Agent Trace"
);
assert(
  composerSource.includes("pendingApproval") && composerSource.includes("composer-permission"),
  "Agent permissions must be actionable from the composer"
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
  "Send and stop must share the primary control while errors expose retry and dismiss actions"
);
assert(
  rustLib.includes("pause_agent_loop_for_control_stop") &&
    rustLib.includes("resume_suspended_agent_run") &&
    rustLib.includes("MAX_AGENT_MODEL_TRANSPORT_ATTEMPTS") &&
    rustLib.includes("is_transient_model_transport_error") &&
    rustLib.includes('"continuation_available".to_string()') &&
    tauriBridge.includes("canContinue: boolean"),
  "Safety-budget stops must retain resumable state and expose a continuation action"
);
assert(
  composerSource.includes('className="composer-toolbar"') &&
    composerSource.includes('className="composer-toolbar-actions"') &&
    styles.includes("width: 36px;") &&
    styles.includes("height: 36px;") &&
    styles.includes("border-radius: var(--radius-round);") &&
    /\.composer-primary-button\.stop:hover \{[\s\S]*?background: #e05b5b;[\s\S]*?filter: none;/.test(styles) &&
    styles.includes('.composer-primary-button.stop:hover .composer-working-ring'),
  "Composer controls must share a bottom toolbar with a circular primary action"
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
  sidebarSource.includes("settingsButtonRef") &&
    sidebarSource.includes('icon.getAnimations().forEach((animation) => animation.cancel())') &&
    sidebarSource.includes('{ transform: "rotate(360deg)" }') &&
    sidebarSource.includes('window.matchMedia("(prefers-reduced-motion: reduce)")'),
  "Settings must rotate once per click and respect reduced-motion preferences"
);
assert(
  sidebarSource.includes('if (!state || (active && state !== "working")) return null;') &&
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
  appSource.includes("matchingSessionExists") &&
    appSource.includes("if (normalizedSidebarQuery)") &&
    sidebarSource.includes("searchActive") &&
    sidebarSource.includes("projectSessions = sessions.filter") &&
    sidebarSource.includes("selectSessionResult") &&
    sidebarSource.includes('"No results"'),
  "Sidebar search must expose matching sessions across projects and navigate to a selected result"
);
assert(
  (appSource.match(/<span>Back to App<\/span>/g)?.length ?? 0) === 1 &&
    !appSource.includes('className="workspace-page-navigation"') &&
    appSource.includes('className="settings-sidebar"') &&
    appSource.includes('className="workspace-return-button settings-app-return"') &&
    styles.includes(".workspace-return-button"),
  "Settings must expose the only in-page return to the app"
);
assert(
  sidebarSource.includes('icons/icon.png') && sidebarSource.includes("appIconUrl"),
  "Sidebar brand must use the high-resolution packaged app icon"
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
    sidebarSource.includes('if (!state || (active && state !== "working")) return null;') &&
    sidebarSource.includes("active={session.active}") &&
    sidebarSource.includes('className="session-name"') &&
    !sidebarSource.includes("<strong>{session.name}</strong>") &&
    styles.includes(".session-name") &&
    appSource.includes("trackedSessionTaskIdsRef") &&
    appSource.includes("markSessionTaskStarted(sessionId)") &&
    appSource.includes('nextStatus = "Completed"') &&
    appSource.includes('nextStatus = "Blocked"'),
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
    sidebarSource.includes('role="alertdialog"') &&
    sidebarSource.includes('aria-modal="true"') &&
    !sidebarSource.includes("window.confirm") &&
    styles.includes(".delete-confirmation-dialog") &&
    appSource.includes("handleDeleteProject") &&
    tauriBridge.includes('invoke<ProjectSessionState>("delete_project"') &&
    rustLib.includes("fn delete_project(") &&
    rustLib.includes("remove_project_from_config"),
  "Project and session menus must use a persisted delete flow with confirmation"
);
assert(
  traceStatusIconSource.includes("CheckCircle2") &&
    traceStatusIconSource.includes("ShieldQuestion") &&
    traceStatusIconSource.includes("XCircle") &&
    inspectorSource.includes('export type InspectorTab = "trace"') &&
    inspectorSource.includes('(["trace", "details", "artifacts", "context"]') &&
    inspectorSource.includes("sessionTraceSteps.map((step)") &&
    inspectorSource.includes("onTraceExport") &&
    styles.includes("grid-template-columns: repeat(4, minmax(0, 1fr));") &&
    inspectorSource.includes("<TraceStatusIcon status={agentStatus}") &&
    inspectorSource.includes("<TraceStatusIcon status={step.status}") &&
    inspectorSource.includes("<TraceStatusIcon status={traceStep.status}") &&
    !appSource.includes('className="trace-view"') &&
    !sidebarSource.includes('aria-label="Agent trace"'),
  "Agent trace must live inside the Inspector Debug drawer and use accessible status icons"
);
assert(
  sidebarSource.includes('aria-label="Create project"') &&
    sidebarSource.includes('title="Create project"') &&
    inspectorSource.includes('aria-label={`Preview ${artifactName(displayPath)}${') &&
    inspectorSource.includes('title={`Preview ${artifactName(displayPath)}${') &&
    composerSource.includes('aria-label={canStop ? "Stop agent" : "Send message"}') &&
    composerSource.includes('title={canStop ? "Stop" : "Send"}'),
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
assert(appSource.includes('id: "runtime"'), "Settings must expose Runtime");
assert(appSource.includes('id: "permissions"'), "Settings must expose Permissions");
assert(
  appSource.includes('id: "about"') &&
    appSource.includes('data-settings-group="about"') &&
    appSource.includes("runtime?.appVersion") &&
    appSource.includes("about-app"),
  "Settings must expose an About page with the packaged runtime version"
);
assert(
  appSource.includes('className="settings-saved-toast"') &&
    appSource.includes("showSettingsSaved();") &&
    (appSource.match(/showSettingsSaved\(\);/g)?.length ?? 0) === 5 &&
    appSource.includes("<CheckCircle2 aria-hidden=\"true\" />") &&
    styles.includes(".settings-saved-toast") &&
    styles.includes("color: #2f9e64;"),
  "Every explicit Settings save must show one green SVG Saved toast"
);
assert(
  appSource.includes("<dt>Created by</dt>") && appSource.includes("<dd>Dale, 2026</dd>"),
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
  appSource.includes('id: "agent"') &&
    appSource.includes("Agent instructions") &&
    appSource.includes("Custom instructions") &&
    appSource.includes("cannot replace permission or") &&
    appSource.includes("providerDraft.agentSystemPrompt") &&
    tauriBridge.includes("agentSystemPrompt: string") &&
    rustLib.includes("agent_system_prompt_hex") &&
    rustLib.includes("model_request_for_turn_with_context") &&
    agentRuntimeSource.includes("compose_base_agent_system_prompt") &&
    coreAgentPrompt.includes("Cindx core contract") &&
    coreAgentPrompt.includes("Verify the requested result with direct evidence"),
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
    inspectorSource.includes("sessionArtifactPaths") &&
    inspectorSource.includes('key.startsWith("result_")') &&
    inspectorSource.includes('step.toolName === "file.write"') &&
    inspectorSource.includes("getAgentSessionOutputs") &&
    inspectorSource.includes("outputHistoryBySession") &&
    inspectorSource.includes("mergeOutputArtifacts") &&
    inspectorSource.includes("currentRunOutputs") &&
    inspectorSource.includes("}, [sessionId]);") &&
    tauriBridge.includes('invoke<AgentOutputArtifactView[]>("get_agent_session_outputs"') &&
    rustLib.includes("get_agent_session_outputs") &&
    rustLib.includes("agent_output_artifacts_from_events") &&
    toolsSource.includes('join("output-history")') &&
    toolsSource.includes('metadata.insert(\n            "artifact_path"') &&
    !inspectorSource.includes("contextCheckpoint?.artifacts.forEach") &&
    !inspectorSource.includes("ragSources.forEach") &&
    !inspectorSource.includes("browserObservations.forEach") &&
    appSource.includes("agentTraceState?.sessionId === activeSession.id") &&
    appSource.includes("sessionTraceSteps={activeSessionTraceSteps}") &&
    styles.includes('.inspector-output-detail[data-fullscreen="true"]') &&
    styles.includes("position: fixed;") &&
    styles.includes("z-index: 100;"),
  "Output previews must retain session history, preserve file versions, and stay scoped to the active session"
);
assert(
  /\.inspector-debug::before \{[\s\S]*?height: calc\(var\(--inspector-debug-panel-height\) \+ 44px\);[\s\S]*?clip-path: inset\(calc\(100% - 44px\) 0 0 0\);[\s\S]*?backdrop-filter: blur\(18px\) saturate\(1\.08\);/.test(styles) &&
    /\.inspector-debug\[data-open="true"\]::before \{[\s\S]*?clip-path: inset\(0\);/.test(styles) &&
    /\.inspector-debug-body \{[\s\S]*?right: 0;[\s\S]*?bottom: 44px;[\s\S]*?left: 0;[\s\S]*?background: transparent;[\s\S]*?border: 0;/.test(styles) &&
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
  "The debug drawer must attach to its bar with a centered, state-correct trailing chevron"
);
assert(
  tauriBridge.includes('invoke<string>("read_artifact_image"') &&
    rustLib.includes("fn read_artifact_image") &&
    rustLib.includes("canonical_path.starts_with(&canonical_root)") &&
    rustLib.includes("24 * 1024 * 1024"),
  "Artifact image previews must stay inside the workspace and enforce a size limit"
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
assert(appSource.includes("<ModelSelect"), "Provider models must use select controls");
assert(appSource.includes("listProviderModels"), "Provider settings must load the remote model catalog");
assert(
  appSource.includes('label="Conductor"') &&
    appSource.includes("providerDraft.conductorModel") &&
    tauriBridge.includes("conductorModel: string") &&
    rustLib.includes("conductor_model: String") &&
    rustLib.includes("model_for_conductor"),
  "Models settings must persist and use a dedicated Conductor model"
);
assert(
  appSource.includes("context-usage") &&
    /\.topbar-actions \{[\s\S]*?gap: 12px;/.test(styles) &&
    /\.context-usage \{[\s\S]*?width: 132px;/.test(styles) &&
    /\.context-usage progress \{[\s\S]*?width: 124px;[\s\S]*?height: 2px;[\s\S]*?border-radius: var\(--radius-pill\);/.test(
      styles
    ),
  "Topbar must expose compact rounded context usage with breathing room before runtime state"
);
assert(
  appSource.includes('activeView !== "settings"') &&
    styles.includes(".topbar-title") &&
    styles.includes(".topbar-actions") &&
    !styles.includes("--titlebar-content-offset-y"),
  "Chat title and status controls must remain hidden in Settings and optically aligned elsewhere"
);
assert(
  appSource.includes('className="settings-tabs"') &&
    appSource.includes('aria-current={settingsCategory === category.id ? "page" : undefined}') &&
    styles.includes(".settings-tabs > button.active span") &&
    !appSource.includes('aria-label="Back to Settings"'),
  "Settings must keep a persistent vertical category tab list with a bold active title"
);
assert(
  appSource.includes("window-workspace-header") && !appSource.includes('className="topbar"'),
  "Session title, context usage, and runtime status must be integrated into the window titlebar"
);
assert(
  appSource.includes("Default effort") &&
    appSource.includes("Cindx Fast") &&
    appSource.includes("Cindx Auto") &&
    appSource.includes("Cindx Pro"),
  "Provider settings must expose the three Cindx effort modes"
);
assert(appSource.includes("Archived sessions"), "Settings must expose archived session recovery");
assert(
  (appSource.match(/className="runtime-state-value"/g)?.length ?? 0) === 3 &&
    appSource.includes('data-state={sidecarState?.autoConfigure ? "auto" : "manual"}') &&
    /\.runtime-state-value\[data-state="ready"\],[\s\S]*?color: #2f9e64;/.test(styles),
  "Runtime Ready and Auto states must use green SVG status indicators"
);
assert(
  appSource.includes("skillRefreshTurn") &&
    appSource.includes("providerModelsRefreshTurn") &&
    (appSource.match(/settings-refresh-turn/g)?.length ?? 0) >= 2 &&
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
  !appSource.includes("Local folder") &&
    appSource.includes(".skill package") &&
    appSource.includes("installSkillUrl") &&
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
  appSource.includes("Knowledge sources") &&
    appSource.includes("knowledge-results") &&
    appSource.includes("phase7.stats.chunksIndexed === 0") &&
    !appSource.includes("Test retrieval") &&
    tauriBridge.includes("if (isTauriRuntime()) throw error"),
  "Knowledge search must auto-index, expose results inline, and surface real Tauri errors"
);
assert(
  appSource.includes("Web search API") &&
    appSource.includes("Save web search") &&
    appSource.includes("registered-tools-details") &&
    tauriBridge.includes('invoke<WebSearchConfigState>("save_web_search_config"') &&
    rustLib.includes("fn save_web_search_config(") &&
    rustLib.includes("web-search.conf") &&
    rustLib.includes("with_workspace_tools_and_services") &&
    toolsSource.includes("fetch_search_api") &&
    toolsSource.includes('"Authorization: Bearer {}"'),
  "Tools settings must persist a private custom web search API and expand built-in tool details"
);
assert(
  appSource.includes('label="Image generation"') &&
    appSource.includes("Image API endpoint") &&
    appSource.includes("provider-endpoint-check") &&
    appSource.includes("validateImageEndpoint") &&
    appSource.includes('emptyLabel="Not configured"') &&
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
  appSource.includes("Pending Reviews") &&
    appSource.includes("permission-review-list") &&
    appSource.includes("handleResolvePermissionReview") &&
    appSource.includes("handleIgnorePermissionReview") &&
    appSource.includes("review.sessionName") &&
    tauriBridge.includes("getPermissionReviewState") &&
    rustLib.includes("fn get_permission_review_state(") &&
    rustLib.includes("struct PermissionReviewItem"),
  "Permission settings must present actionable reviews with source session context"
);
assert(
  styles.includes(".advanced-settings summary::-webkit-details-marker") &&
    appSource.includes('className="settings-disclosure-chevron"') &&
    styles.includes(".settings-disclosure-chevron"),
  "Expandable settings must use a consistent trailing chevron"
);
assert(
  appSource.includes('className="settings-sidebar"') &&
    appSource.includes('className="settings-detail"') &&
    !appSource.includes("settings-index") &&
    styles.includes("grid-template-columns: 168px minmax(0, 760px)") &&
    /\.settings-app-return \{[\s\S]*?top: -12px;/.test(styles),
  "Settings must use persistent left tabs and right-side details"
);
assert(
  rustLib.includes("maybe_auto_name_session") &&
    rustLib.includes("semantic_session_title") &&
    rustLib.includes("automatic_conversation_title") &&
    rustLib.includes("First assistant response") &&
    rustLib.includes("can_apply_generated_session_title") &&
    appSource.includes("sessionTitleFromPrompt") &&
    appSource.includes("sessionTitleFromFirstRound") &&
    appSource.includes("refineAutomaticSessionTitle") &&
    appSource.includes("firstRoundAnswer") &&
    tauriBridge.includes('invoke<ProjectSessionState>("generate_session_title"'),
  "New sessions must receive a non-blocking semantic title without overwriting manual names"
);
assert(
  styles.includes(".lucide-check") &&
    styles.includes(".lucide-circle-check") &&
    styles.includes("color: var(--accent) !important") &&
    styles.includes("backdrop-filter: saturate(155%) blur(24px)") &&
    inspectorSource.includes("PackageOpen") &&
    inspectorSource.includes("<PackageOpen aria-hidden=\"true\" />"),
  "Checkmarks, sidebar material, and the Outputs heading icon must retain their visual treatment"
);
assert(
  appSource.includes("busy={projectSessionBusy}") &&
    appSource.includes("busySessionIds") &&
    appSource.includes("composerDrafts") &&
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
  composerSource.includes("const EFFORT_OPTIONS") &&
    composerSource.includes('label: "Cindx Fast"') &&
    composerSource.includes('description: "One model for quick, focused tasks"') &&
    composerSource.includes('label: "Cindx Auto"') &&
    composerSource.includes('description: "Routes each request by complexity"') &&
    composerSource.includes('label: "Cindx Pro"') &&
    composerSource.includes('description: "Multi-model collaboration for hard tasks"') &&
    composerSource.includes('className="composer-effort-menu"') &&
    composerSource.includes('role="listbox"') &&
    composerSource.includes('role="option"') &&
    appSource.includes('useState<AgentEffort>("auto")') &&
    tauriBridge.includes('export type AgentEffort = "fast" | "auto" | "pro"') &&
    tauriBridge.includes("currentTime: currentAgentTimeContext(), effort, attachments") &&
    rustLib.includes("enum AgentEffort") &&
    rustLib.includes('"agent_effort".to_string()') &&
    rustLib.includes("agent_effort_from_active_events"),
  "Composer effort must map Cindx Fast, Auto, and Pro through retries and traces"
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
    "session_permission_grant_covers_non_destructive_requests_in_the_same_session"
  ) &&
    rustLib.includes(
      ".filter(|pending| !matches!(&pending.risk, PermissionRisk::Destructive))"
    ) &&
    rustLib.includes("destructive permissions can only be allowed once"),
  "Allow session must cover future non-destructive requests without covering destructive tools"
);
assert(
  rustLib.includes("execute_agent_tool_invocation") &&
    rustLib.includes("drop(store);") &&
    appSource.includes("markSessionBusy"),
  "Agent tool execution must release the shared event-store lock"
);
assert(
  rustLib.includes('join("Application Support").join("Cindx")') &&
    rustLib.includes("persistent state unavailable; using in-memory state") &&
    rustLib.includes("install_startup_panic_log") &&
    rustLib.includes('std::env::var("CINDX_STARTUP_PROBE")') &&
    ciWorkflow.includes("Probe clean-machine startup") &&
    !rustLib.includes('workspace_root().join(".cindx").join("state.sqlite3")'),
  "Installed apps must use user-scoped data and survive persistent-state failures"
);
assert(
  rustLib.includes("EVENT_REDACTION_MARKER_FILE") &&
    rustLib.includes("event_redaction_pending && persistent_store") &&
    rustLib.includes("event_redaction_marker_records_completed_migration"),
  "Legacy event redaction must be a versioned one-time startup migration"
);
assert(
  appSource.includes("agentStateUnchanged") &&
    appSource.includes("agentTraceUnchanged") &&
    appSource.includes("if (current[sessionId] === nextStatus) return current"),
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
    promptEvolutionSource.includes("format_valid_rate < 1.0") &&
    rustLib.includes("pareto_search_teacher_v2") &&
    rustLib.includes("prompt_evolution_enabled") &&
    rustLib.includes("prompt_evolution_evaluation_for_run") &&
    rustLib.includes('"prompt_evolution_mutation"') &&
    rustLib.includes("PROMPT_EVOLUTION_STAGNATION_PATIENCE") &&
    rustLib.includes("PROMPT_EVOLUTION_SHADOW_INTERVAL") &&
    rustLib.includes('"Agent task completed" | "Agent task cancelled" | "Agent task failed"') &&
    rustLib.includes("evaluate_prompt_evolution") &&
    rustLib.includes("Conductor prompt profile selected") &&
    rustLib.includes("PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS") &&
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
    rustLib.includes('"Conductor prompt rollout updated"') &&
    rustLib.includes('"evaluation_required"') &&
    tauriBridge.includes("setPromptEvolutionEnabled") &&
    tauriBridge.includes("averageRelativeReward") &&
    tauriBridge.includes("averageStepCredit") &&
    tauriBridge.includes("promotionConfidence") &&
    tauriBridge.includes("canaryPercent") &&
    appSource.includes('if (category === "tools") return <Wrench aria-hidden="true" />;') &&
    /<Wrench size=\{17\} aria-hidden="true" \/>\s*<h2>Tools<\/h2>/.test(appSource) &&
    /<Dna size=\{17\} aria-hidden="true" \/>\s*<h2>Genetic Pareto<\/h2>/.test(appSource) &&
    appSource.includes("Genetic Pareto") &&
    appSource.includes("Candidate harnesses execute in an isolated arena before promotion") &&
    appSource.includes("Wilson confidence gate controls staged canary rollout") &&
    appSource.includes("Evaluating in background") &&
    appSource.includes("Rollout by effort") &&
    appSource.includes("Candidate profiles") &&
    appSource.includes("prompt-evolution-table") &&
    appSource.includes("prompt-evolution-summary") &&
    appSource.includes("Rollbacks") &&
    styles.includes(".prompt-evolution-table") &&
    !appSource.includes("prompt-evolution-efforts") &&
    !appSource.includes("prompt-evolution-profiles"),
  "Conductor workflows must run executable harness evolution with confidence-gated canary rollout"
);

const nonGrayColors = [...styles.matchAll(/#([0-9a-fA-F]{6})(?![0-9a-fA-F])/g)]
  .map((match) => match[1].toLowerCase())
  .filter(
    (hex) =>
      ![
        "2563eb",
        "2f9e64",
        "39b96b",
        "9fe3b0",
        "e05b5b",
        "eef6ff",
        "e3efff"
      ].includes(hex)
  )
  .filter((hex) => hex.slice(0, 2) !== hex.slice(2, 4) || hex.slice(2, 4) !== hex.slice(4, 6));
assert(
  nonGrayColors.length === 0,
  "Desktop theme must remain grayscale except for brand, message, and session-state accents"
);
assert(appSource.includes("Pending Reviews"), "App must render pending permission reviews");
assert(
  !appSource.includes("<h2>Orchestration</h2>") &&
    !appSource.includes("Manual workflow test") &&
    tauriBridge.includes('invoke<Phase6State>("run_orchestration"'),
  "Manual orchestration must stay out of user settings while remaining available to diagnostics"
);
assert(appSource.includes("Provider"), "App must render provider UI");
assert(appSource.includes("Save workspace"), "App must render workspace save action");
assert(
  appSource.includes("handlePickWorkspace") &&
    appSource.includes("workspace-folder-selector") &&
    appSource.includes("pickWorkspaceFolder"),
  "Workspace settings must use the native folder selector"
);
assert(
  sidebarSource.includes("Folders") &&
    sidebarSource.includes("nav-heading-with-icon") &&
    styles.includes(".nav-heading-with-icon"),
  "Projects heading must render an aligned SVG icon"
);
assert(appSource.includes("Save provider"), "App must render provider save action");
assert(appSource.includes("Run tool"), "App must render the Phase 5 tool runner");
assert(appSource.includes("Index workspace"), "App must render the Phase 7 RAG index action");
assert(appSource.includes("answerWithRag"), "App must render the Phase 7 RAG answer flow");
assert(appSource.includes("Search web"), "App must render the Phase 8 web search action");
assert(appSource.includes("runBrowserTool"), "App must render the Phase 8 browser flow");
assert(
  appSource.includes('handleRunBrowserTool("browser.tabs")') &&
    appSource.includes('handleRunBrowserTool("browser.select_tab")'),
  "Browser settings must expose tab listing and selection"
);
assert(appSource.includes("getRuntimeStatus"), "App must call the runtime bridge");
assert(
  !appSource.includes("Request review") && appSource.includes("Approve once"),
  "Permission settings must review real pending actions instead of creating mock requests"
);

assert(
  tauriBridge.includes('invoke<RuntimeStatus>("get_runtime_status")'),
  "Frontend bridge must invoke get_runtime_status"
);
assert(
  tauriBridge.includes('invoke<RuntimeStatus>("save_workspace_root"'),
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
    tauriBridge.includes('invoke<AgentAttachment[]>("stage_agent_attachments"') &&
    rustLib.includes("fn stage_agent_attachments(") &&
    rustLib.includes("validated_attachment_path"),
  "Composer attachments must be staged and validated inside the active project"
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
    rustLib.includes("Agent task resumed after permission"),
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
  rustLib.includes("run_parallel_retrieval(") &&
    rustLib.includes('timed_retrieval_channel("semantic_rag"') &&
    rustLib.includes('timed_retrieval_channel("graph_recall"') &&
    rustLib.includes('timed_retrieval_channel("graph_walk"') &&
    rustLib.includes('timed_retrieval_channel("file_search"') &&
    rustLib.includes('retrieval_mode == "four_way_parallel"') &&
    rustLib.includes("coding_retrieval_mode_skips_graph_channels") &&
    rustLib.includes("fuse_retrieval_channels") &&
    rustLib.includes("prepare_agent_knowledge_context") &&
    ragSource.includes("search_chunks_semantic") &&
    ragSource.includes("search_chunks_literal"),
  "Knowledge retrieval must select two or four channels, fuse results, and feed the agent"
);
assert(
  appSource.includes("Graph Explorer") &&
    knowledgeGraphSource.includes('aria-label="Workspace knowledge graph"') &&
    knowledgeGraphSource.includes('from "d3-force"') &&
    knowledgeGraphSource.includes("forceSimulation(positioned)") &&
    knowledgeGraphSource.includes("forceLink<PositionedNode, SimulationEdge>") &&
    knowledgeGraphSource.includes("graph.edges.filter") &&
    knowledgeGraphSource.includes('data-muted={Boolean(activeId)') &&
    styles.includes(".knowledge-graph-node-label") &&
    styles.includes('.knowledge-graph-edges line[data-muted="true"]'),
  "Knowledge settings must expose an Obsidian-style force-directed graph backed by graph state"
);
assert(
  rustLib.includes("context_checkpoint_path_for_session") &&
    rustLib.includes("prepare_session_history_context") &&
    rustLib.includes("SessionCompactionPlan") &&
    rustLib.includes('"hybrid_v2"') &&
    rustLib.includes("recent_history_start") &&
    agentMemorySource.includes("conversation_memory_to_markdown") &&
    rustLib.includes("Session context restored for agent run") &&
    rustLib.includes("event_matches_context"),
  "Context compaction must preserve session-scoped operational and conversational memory"
);
assert(
  rustLib.includes("synthesize_agent_answer(") &&
    rustLib.includes("run_adaptive_collaboration(") &&
    orchestratorSource.includes("pub struct ConductorHarness") &&
    orchestratorSource.includes("pub fn planning_prompt(&self)") &&
    orchestratorSource.includes("pub fn repair_prompt(") &&
    orchestratorSource.includes("pub fn parse_plan(") &&
    rustLib.includes("CONDUCTOR_MAX_ATTEMPTS") &&
    rustLib.includes("run_collaboration_candidates(") &&
    rustLib.includes("complete_collaboration_worker_with_tools(") &&
    rustLib.includes('"isolated_evidence_v1"') &&
    agentRuntimeSource.includes("evidence_worker_tools") &&
    agentRuntimeSource.includes("DEFAULT_COLLABORATION_WORKER_TURNS") &&
    rustLib.includes("std::thread::spawn") &&
    rustLib.includes('"conductor_plan"') &&
    rustLib.includes('format!("worker_{}", step_index + 1)') &&
    rustLib.includes('("access_list".to_string(), spec.access.join(","))') &&
    rustLib.includes('"arbiter"') &&
    rustLib.includes('"planner"') &&
    rustLib.includes('"reviewer"') &&
    rustLib.includes('"synthesizer"') &&
    rustLib.includes("recover_adaptive_worker(") &&
    rustLib.includes("quality_gate_adaptive_output(") &&
    rustLib.includes("append_single_model_policy_guidance(") &&
    rustLib.includes("let OrchestrationPolicy::BestOfN { candidates } = policy else") &&
    rustLib.includes("route_with_local_telemetry(") &&
    orchestratorSource.includes("MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5") &&
    orchestratorSource.includes("MAX_ADAPTIVE_WORKFLOW_AGENTS: usize = 3") &&
    orchestratorSource.includes("adaptive_workflow_step_budget") &&
    orchestratorSource.includes("the final adaptive workflow must incorporate every branch") &&
    orchestratorSource.includes("complexity_score") &&
    orchestratorSource.includes("estimated_steps") &&
    orchestratorSource.includes("parallelizable") &&
    orchestratorSource.includes("verification_required") &&
    orchestratorSource.includes("latency_sensitive") &&
    orchestratorSource.includes('"rule_based_v2"') &&
    orchestratorSource.includes('"learned_conductor_v1"') &&
    orchestratorSource.includes('"collaboration_budget"') &&
    orchestratorSource.includes("high_stakes") &&
    orchestratorSource.includes('"thinker" | "worker" | "verifier" | "synthesizer"') &&
    orchestratorSource.includes("adaptive_workflow_layers") &&
    orchestratorSource.includes("adaptive_worker_prompt") &&
    orchestratorSource.includes("ordinary_research_uses_one_planned_execution_path") &&
    orchestratorSource.includes("latency_sensitive_complex_request_does_not_spawn_an_ensemble") &&
    orchestratorSource.includes("auto_router_selects_models_by_task_role") &&
    orchestratorSource.includes("learned_router_cannot_upgrade_ordinary_research_to_ultra") &&
    rustLib.includes("AgentEffort::Auto if !routing_decision.model.trim().is_empty()") &&
    orchestratorSource.includes("must only access earlier steps") &&
    rustLib.includes('"conductor_version".to_string(), "agent_v2".to_string()') &&
    rustLib.includes('"workflow_ir".to_string()') &&
    orchestratorSource.includes('WORKFLOW_IR_SCHEMA: &str = "cindx.workflow.v1"') &&
    orchestratorSource.includes("WorkflowSearchTeacher") &&
    rustLib.includes("Collaboration workflow planned") &&
    rustLib.includes("Tool evidence ledger") &&
    rustLib.includes("collaboration_step_result(") &&
    rustLib.includes('"evidence_count"') &&
    rustLib.includes("adaptive_coordinator_accepts_five_steps_with_three_reused_models") &&
    rustLib.includes("conductor_result_separates_worker_claims_from_tool_evidence"),
  "Primary agent must reserve bounded tool-capable adaptive workflows for Ultra-routed requests"
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
    agentEvaluationDoc.includes("Versioned Contract Suite") &&
    agentEvaluationDoc.includes("Real Run Observations") &&
    ciWorkflow.includes("--report target/agent-benchmark-report.json") &&
    ciWorkflow.includes("Upload agent benchmark report"),
  "CI must run the versioned offline benchmark and retain auditable quality guidance"
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
assert(rustLib.includes("fn index_workspace_rag("), "Phase 7 index command is missing");
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
    browserControlDoc.includes("CDP owns browser process discovery") &&
    ciWorkflow.includes("Test Browser Control v2") &&
    localBuildScript.includes("test-browser-sidecar.mjs"),
  "Browser Control v2 must combine CDP transport, Playwright semantics, and a real-browser gate"
);

console.log("desktop structure ok");
