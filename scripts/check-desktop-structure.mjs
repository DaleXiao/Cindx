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
const traceStatusIconSource = read("apps/desktop/src/components/TraceStatusIcon.tsx");
const styles = read("apps/desktop/src/styles.css");
const tauriBridge = read("apps/desktop/src/tauri.ts");
const rustLib = read("apps/desktop/src-tauri/src/lib.rs");
const cargoToml = read("apps/desktop/src-tauri/Cargo.toml");
const cargoLock = read("apps/desktop/src-tauri/Cargo.lock");
const runTauriSource = read("scripts/run-tauri.mjs");
const releaseWorkflow = read(".github/workflows/release.yml");
const unsignedReleaseStart = releaseWorkflow.indexOf(
  "- name: Build and publish unsigned Universal app"
);
const unsignedReleaseBlock =
  unsignedReleaseStart >= 0 ? releaseWorkflow.slice(unsignedReleaseStart) : "";
const ciWorkflow = read(".github/workflows/ci.yml");
const releaseVersionCheck = read("scripts/check-release-version.mjs");
const toolsSource = read("crates/tools/src/lib.rs");
const ragSource = read("crates/agent-rag/src/lib.rs");
const orchestratorSource = read("crates/orchestrator/src/lib.rs");
const mainSource = read("apps/desktop/src/main.tsx");
const html = read("apps/desktop/index.html");
const viteConfig = read("apps/desktop/vite.config.ts");

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
  releaseVersionCheck.includes("does not match committed version"),
  "release workflow must reject mismatched tags"
);
assert(packageJson.dependencies.react, "React dependency is missing");
assert(packageJson.dependencies["@tauri-apps/api"], "Tauri API dependency is missing");
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
const macOSTrafficLightSize = 14;
const centeredTrafficLightTop = (titlebarHeight - macOSTrafficLightSize) / 2;

assert(
  tauriConfig.app.windows.every(
    (window) => window.trafficLightPosition?.y === centeredTrafficLightTop
  ) &&
    styles.includes(`--titlebar-height: ${titlebarHeight}px`) &&
    styles.includes("--titlebar-control-size: 28px") &&
    styles.includes("grid-template-rows: var(--titlebar-height) minmax(0, 1fr)") &&
    styles.includes(
      "top: calc((var(--titlebar-height) - var(--titlebar-control-size)) / 2)"
    ) &&
    !styles.includes("--titlebar-content-offset-y"),
  "Native traffic lights and custom titlebar controls must retain their optical alignment"
);
assert(
  tauriConfig.bundle.icon.includes("icons/icon.icns"),
  "Tauri bundle must use the generated macOS app icon"
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
  sessionThreadSource.includes('aria-label="Session thread"'),
  "Session thread must expose its semantic region"
);
assert(
  packageJson.dependencies["markdown-to-jsx"] &&
    sessionThreadSource.includes('from "markdown-to-jsx"') &&
    sessionThreadSource.includes("disableParsingRawHTML: true") &&
    sessionThreadSource.includes("<AgentMarkdown content={item.message.content}") &&
    sessionThreadSource.includes("<AgentMarkdown content={streamAnswer} streaming") &&
    styles.includes(".thread-markdown pre code") &&
    styles.includes(".thread-markdown table"),
  "Assistant messages must render safe Markdown in completed and streaming states"
);
assert(!appSource.includes("Agent Output"), "Primary model output must not be duplicated in the inspector");
assert(
  inspectorSource.includes('"details", "artifacts", "context"'),
  "Inspector must expose Details, Artifacts, and Context"
);
assert(inspectorSource.includes("inspector-resize-handle"), "Inspector must remain resizable");
assert(
  !inspectorSource.includes("<strong>Inspector</strong>") &&
    !styles.includes(".inspector-header"),
  "Inspector tabs must lead the panel without a redundant visible title"
);
assert(
  appSource.includes('aria-label={inspectorOpen ? "Hide inspector" : "Show inspector"}') &&
    !inspectorSource.includes("Close inspector"),
  "Inspector must use one stable toggle instead of duplicate controls"
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
  styles.includes("--icon-radius: 9px") && styles.includes("window-toolbar::before"),
  "Icon feedback and full-height pane dividers must retain their polished geometry"
);
assert(
  appSource.includes("window-toolbar-panel-left") &&
    appSource.includes("window-toolbar-panel-right") &&
    styles.includes("backdrop-filter: saturate(1.3) blur(18px)") &&
    styles.includes("background: rgba(250, 250, 250, 0.62)") &&
    tauriConfig.app.macOSPrivateApi === true &&
    tauriConfig.app.windows.every((window) => window.transparent === true) &&
    cargoToml.includes('features = ["macos-private-api"]') &&
    cargoToml.includes('window-vibrancy = "0.6.0"') &&
    rustLib.includes("window_vibrancy::apply_vibrancy") &&
    rustLib.includes("NSVisualEffectMaterial::HeaderView"),
  "Titlebar must expose the native macOS material through a translucent web layer"
);
assert(composerSource.includes('event.key !== "Enter"'), "Composer must support Enter to send");
assert(composerSource.includes("event.shiftKey"), "Composer must reserve Shift+Enter for a new line");
assert(
  composerSource.includes("onCompositionStart") &&
    composerSource.includes("onCompositionEnd") &&
    composerSource.includes("compositionJustEndedRef") &&
    composerSource.includes("nativeEvent.keyCode === 229"),
  "Composer must not submit macOS IME candidate-selection keystrokes"
);
assert(
  sessionThreadSource.includes("thread.scrollTop = thread.scrollHeight"),
  "Session thread must follow the latest output"
);
assert(
  sessionThreadSource.includes('className="thread-minimap"') &&
    sessionThreadSource.includes('role="scrollbar"') &&
    sessionThreadSource.includes("onPointerMove={handleMinimapPointerMove}"),
  "Session thread must expose an interactive left-side minimap"
);
assert(
  sessionThreadSource.includes("minimapMarkerPosition") &&
    sessionThreadSource.includes("MINIMAP_MARKER_GAP = 14") &&
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
    sessionThreadSource.includes("marker.targetIndex"),
  "Minimap must index only sparse, substantive user and model output anchors"
);
assert(
  styles.includes(".session-thread::-webkit-scrollbar") &&
    styles.includes("scrollbar-width: none") &&
    styles.includes(".thread-minimap-position") &&
    styles.includes('.thread-minimap-marker[data-wave-distance="0"]'),
  "Session thread must replace its native scrollbar with the minimap"
);
assert(
  sessionThreadSource.includes("thread-message-actions") &&
    sessionThreadSource.includes("onEditMessage"),
  "User messages must expose copy and edit actions"
);
assert(
  sessionThreadSource.includes("thread-message-agent-meta") &&
    sessionThreadSource.includes("!isUser && !isAssistant") &&
    sessionThreadSource.includes("thread-streaming-status") &&
    !sessionThreadSource.includes('<strong>Cindx</strong>'),
  "Assistant output must begin without a robot icon or Cindx label"
);
assert(
  sessionThreadSource.includes("groupThreadItems") &&
    sessionThreadSource.includes("containsToolActivity") &&
    sessionThreadSource.includes("thread-tool-chain") &&
    sessionThreadSource.includes("Agent activity") &&
    sessionThreadSource.includes("<ToolChainItem") &&
    styles.includes(".thread-tool-chain-items"),
  "A complete agent activity chain must default to one parent disclosure"
);
assert(
  disclosureTriangleSource.includes('import { Triangle } from "lucide-react"') &&
    (sessionThreadSource.match(/<DisclosureTriangle/g)?.length ?? 0) >= 3 &&
    (inspectorSource.match(/<DisclosureTriangle/g)?.length ?? 0) >= 2 &&
    (appSource.match(/<DisclosureTriangle/g)?.length ?? 0) >= 5 &&
    !sessionThreadSource.includes("ChevronRight") &&
    !inspectorSource.includes("ChevronRight") &&
    !styles.includes("advanced-settings summary::before") &&
    styles.includes("details[open] > summary .disclosure-triangle"),
  "Every disclosure indicator must use the shared equilateral triangle icon"
);
assert(
  sessionThreadSource.includes("thread-thinking") &&
    sessionThreadSource.includes("Thinking") &&
    !sessionThreadSource.includes("LoaderCircle") &&
    styles.includes("@keyframes thinking-sheen") &&
    styles.includes("prefers-reduced-motion: reduce"),
  "Agent thinking state must use a reduced-motion-safe text sheen without a spinner"
);
assert(
  composerSource.includes("pendingApproval") && composerSource.includes("composer-permission"),
  "Agent permissions must be actionable from the composer"
);
assert(
  composerSource.includes("composer-stop-icon") &&
    composerSource.includes("retryMode") &&
    !composerSource.includes("agent-control-button"),
  "Send, stop, and retry must share one primary composer control"
);
assert(
  styles.includes("width: 48px;") &&
    styles.includes("height: 48px;") &&
    styles.includes("border-radius: 14px;"),
  "The primary composer control must use the larger aligned shape"
);
assert(sidebarSource.includes("session-branch"), "Sessions must be nested below the active project");
assert(
  sidebarSource.includes("collapsedProjectIds") &&
    sidebarSource.includes('className="project-disclosure"') &&
    sidebarSource.includes("aria-expanded={expanded}") &&
    sidebarSource.includes("handleProjectDisclosure(project, expanded)") &&
    sidebarSource.includes("<DisclosureTriangle />") &&
    styles.includes('.project-disclosure[aria-expanded="true"] .disclosure-triangle'),
  "Each project must expose an accessible disclosure control for its sessions"
);
assert(
  styles.includes(".project-item") &&
    styles.includes(".session-item") &&
    styles.includes("border-radius: 8px;") &&
    styles.includes(".settings-section {") &&
    styles.includes(".archived-session-row + .archived-session-row"),
  "Project, session, and archived-session geometry must retain the quieter rounded treatment"
);
assert(sidebarSource.includes("sidebar-footer"), "Settings must remain in the sidebar footer");
assert(
  sidebarSource.includes('icons/icon.png') && sidebarSource.includes("appIconUrl"),
  "Sidebar brand must use the high-resolution packaged app icon"
);
assert(!sidebarSource.includes("Local agent"), "Sidebar brand must not show the old subtitle");
assert(
  !sidebarSource.includes("{project.status}") &&
    !sidebarSource.includes("{session.detail}") &&
    !sidebarSource.includes("{session.status}"),
  "Sidebar rows must not show redundant project or session metadata"
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
assert(sidebarSource.includes("onSessionFork"), "Session menu must expose fork");
assert(sidebarSource.includes("onSessionArchive"), "Session menu must expose archive");
assert(sidebarSource.includes("onSessionDelete"), "Session menu must expose delete");
assert(
  traceStatusIconSource.includes("CheckCircle2") &&
    traceStatusIconSource.includes("ShieldQuestion") &&
    traceStatusIconSource.includes("XCircle") &&
    appSource.includes('<TraceStatusIcon status={agentTraceState?.status ?? "idle"}') &&
    appSource.includes("<TraceStatusIcon status={turn.status}") &&
    appSource.includes("<TraceStatusIcon status={step.status}") &&
    inspectorSource.includes("<TraceStatusIcon status={traceStep.status}") &&
    !appSource.includes("<em>{step.status}</em>"),
  "Agent trace statuses must render as accessible SVG icons"
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
    appSource.includes("Agent system prompt") &&
    appSource.includes("providerDraft.agentSystemPrompt") &&
    tauriBridge.includes("agentSystemPrompt: string") &&
    rustLib.includes("agent_system_prompt_hex") &&
    rustLib.includes("model_request_for_turn_with_system_prompt"),
  "Settings must persist and apply an editable Agent system prompt"
);
assert(!styles.includes("artifact-sidebar"), "Legacy artifact sidebar styles must be removed");
assert(
  inspectorSource.includes("inspector-outputs") &&
    inspectorSource.includes("inspector-debug") &&
    inspectorSource.includes("readArtifactImage") &&
    inspectorSource.includes("useState(false)"),
  "Inspector must lead with output previews and keep debug reference views collapsed"
);
assert(
  tauriBridge.includes('invoke<string>("read_artifact_image"') &&
    rustLib.includes("fn read_artifact_image") &&
    rustLib.includes("canonical_path.starts_with(&canonical_root)") &&
    rustLib.includes("24 * 1024 * 1024"),
  "Artifact image previews must stay inside the workspace and enforce a size limit"
);
assert(!tauriBridge.includes("apiKeyPreview"), "Provider state must not expose API key suffixes");
assert(appSource.includes("<ModelSelect"), "Provider models must use select controls");
assert(appSource.includes("listProviderModels"), "Provider settings must load the remote model catalog");
assert(appSource.includes("context-usage"), "Topbar must expose context token usage");
assert(
  appSource.includes('activeView !== "settings"') &&
    styles.includes(".topbar-title") &&
    styles.includes(".topbar-actions") &&
    !styles.includes("--titlebar-content-offset-y"),
  "Chat title and status controls must remain hidden in Settings and optically aligned elsewhere"
);
assert(
  appSource.includes("settings-page-navigation") &&
    appSource.includes('aria-label="Back to Settings"'),
  "Settings detail pages must keep an in-content route back to the category list"
);
assert(
  appSource.includes("window-workspace-header") && !appSource.includes('className="topbar"'),
  "Session title, context usage, and runtime status must be integrated into the window titlebar"
);
assert(
  appSource.includes("Quality synthesis") && appSource.includes("Ensemble deliberation"),
  "Provider settings must expose collaboration quality and ensemble modes"
);
assert(appSource.includes("Archived sessions"), "Settings must expose archived session recovery");
assert(
  appSource.includes("<TriangleAlert size={14}") &&
    styles.includes(".permission-callout > svg") &&
    styles.includes("border: 0;"),
  "Permission settings must use a compact alert icon without a trailing section divider"
);
assert(
  styles.includes(".advanced-settings summary::-webkit-details-marker") &&
    appSource.includes("<DisclosureTriangle />") &&
    styles.includes(".disclosure-triangle"),
  "Expandable settings must use a consistent equilateral disclosure marker"
);
assert(
  appSource.includes("settings-index") && appSource.includes("Back to Settings"),
  "Settings must expose an index page and a detail back action"
);
assert(
  rustLib.includes("maybe_auto_name_session") && appSource.includes("sessionTitleFromPrompt"),
  "New sessions must be named from their first prompt"
);
assert(
  appSource.includes("busy={projectSessionBusy}") &&
    appSource.includes("busySessionIds") &&
    appSource.includes("composerDrafts") &&
    !appSource.includes("agentBusy"),
  "Running one session must not disable navigation to other sessions"
);
assert(
  tauriBridge.includes("runAgentTask(prompt: string, sessionId: string)") &&
    tauriBridge.includes("currentTime: currentAgentTimeContext()") &&
    rustLib.includes("project_session_metadata_for_session") &&
    rustLib.includes("event.metadata.get(\"session_id\")"),
  "Agent commands and event boundaries must remain isolated by session"
);
assert(
  tauriBridge.includes("function currentAgentTimeContext()") &&
    rustLib.includes("normalized_current_time_context") &&
    rustLib.includes("agent_system_prompt_for_run") &&
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

const nonGrayColors = [...styles.matchAll(/#([0-9a-fA-F]{6})(?![0-9a-fA-F])/g)]
  .map((match) => match[1].toLowerCase())
  .filter(
    (hex) => !["2563eb", "2f9e64", "9fe3b0", "f1fff4", "d8f0dd", "afd2b7"].includes(hex)
  )
  .filter((hex) => hex.slice(0, 2) !== hex.slice(2, 4) || hex.slice(2, 4) !== hex.slice(4, 6));
assert(
  nonGrayColors.length === 0,
  "Desktop theme must remain grayscale except for the Cindx blue and user message green"
);
assert(appSource.includes("Permissions"), "App must render permission UI");
assert(appSource.includes("Orchestration"), "App must render orchestration UI");
assert(appSource.includes("Provider"), "App must render provider UI");
assert(appSource.includes("Save workspace"), "App must render workspace save action");
assert(appSource.includes("Save provider"), "App must render provider save action");
assert(appSource.includes("Run tool"), "App must render the Phase 5 tool runner");
assert(appSource.includes("Run workflow"), "App must render the Phase 6 workflow runner");
assert(appSource.includes("Index workspace"), "App must render the Phase 7 RAG index action");
assert(appSource.includes("answerWithRag"), "App must render the Phase 7 RAG answer flow");
assert(appSource.includes("Search web"), "App must render the Phase 8 web search action");
assert(appSource.includes("runBrowserTool"), "App must render the Phase 8 browser flow");
assert(appSource.includes("getRuntimeStatus"), "App must call the runtime bridge");
assert(appSource.includes("Request review"), "App must render the Phase 3 mock review action");

assert(
  tauriBridge.includes('invoke<RuntimeStatus>("get_runtime_status")'),
  "Frontend bridge must invoke get_runtime_status"
);
assert(
  tauriBridge.includes('invoke<RuntimeStatus>("save_workspace_root"'),
  "Frontend bridge must invoke save_workspace_root"
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
assert(rustLib.includes("fn rename_session("), "Session rename command is missing");
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
  rustLib.includes("synthesize_agent_answer(") &&
    rustLib.includes("run_adaptive_collaboration(") &&
    rustLib.includes("parse_adaptive_workflow(") &&
    rustLib.includes("run_collaboration_candidates(") &&
    rustLib.includes("std::thread::spawn") &&
    rustLib.includes('"coordinator"') &&
    rustLib.includes('format!("worker_{}", step_index + 1)') &&
    rustLib.includes('("access_list".to_string(), spec.access.join(","))') &&
    rustLib.includes('"arbiter"') &&
    rustLib.includes('"planner"') &&
    rustLib.includes('"reviewer"') &&
    rustLib.includes('"synthesizer"') &&
    orchestratorSource.includes("MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5") &&
    orchestratorSource.includes("adaptive_workflow_layers") &&
    orchestratorSource.includes("adaptive_worker_prompt") &&
    orchestratorSource.includes("must only access earlier steps"),
  "Primary agent must run bounded, dependency-aware multi-model workflows with isolated context and fallback synthesis"
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
  rustLib.includes("browser.capture") && rustLib.includes("phase8_state"),
  "Rust bridge must connect the Phase 8 browser runtime"
);

console.log("desktop structure ok");
