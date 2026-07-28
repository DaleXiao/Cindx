import fs from "node:fs";

const styleRoot = "apps/desktop/src/styles";
const css = [
  fs.readFileSync("apps/desktop/src/styles.css", "utf8"),
  ...fs
    .readdirSync(styleRoot)
    .filter((entry) => entry.endsWith(".css"))
    .sort()
    .map((entry) => fs.readFileSync(`${styleRoot}/${entry}`, "utf8"))
].join("\n");
const appSource = fs.readFileSync("apps/desktop/src/App.tsx", "utf8");
const workspaceChromeSource = fs.readFileSync(
  "apps/desktop/src/components/WorkspaceChrome.tsx",
  "utf8"
);
const config = JSON.parse(
  fs.readFileSync("apps/desktop/src-tauri/tauri.conf.json", "utf8")
);
const windowConfig = config.app.windows[0];

const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

assert(windowConfig.width === 1280, "Default desktop width must remain 1280px");
assert(windowConfig.height === 820, "Default desktop height must remain 820px");
assert(windowConfig.minWidth === 960, "Minimum desktop width must remain 960px");
assert(windowConfig.titleBarStyle === "Overlay", "macOS title bar must use the overlay layout");
assert(windowConfig.hiddenTitle === true, "macOS title text must remain hidden");
assert(
  css.includes("--sidebar-layout-width: var(--sidebar-width, 236px)") &&
    css.includes(
      "grid-template-columns: var(--sidebar-layout-width) minmax(0, 1fr) var(--inspector-layout-width)"
    ),
  "Wide layout must use resizable three-pane columns"
);
assert(
  css.includes("--titlebar-height: 46px") &&
    css.includes("grid-template-rows: var(--titlebar-height) minmax(0, 1fr)"),
  "Layout must reserve a custom titlebar row"
);
assert(css.includes("@media (max-width: 1180px)"), "Compact desktop breakpoint is missing");
assert(css.includes('position: fixed;\n    z-index: 20;'), "Compact inspector must become an overlay");
assert(
  /\.window-workspace-header \{[\s\S]*?grid-column: 2;[\s\S]*?grid-row: 1;/.test(css) &&
    /\.workspace \{[\s\S]*?grid-column: 2;[\s\S]*?grid-row: 2;/.test(css) &&
    workspaceChromeSource.indexOf("</header>") <
      workspaceChromeSource.indexOf('<div className="window-workspace-header">') &&
    appSource.indexOf("<WorkspaceChrome") <
      appSource.indexOf('<section className="workspace"'),
  "Workspace title and content must share the same responsive grid column"
);
assert(css.includes('.app-shell[data-inspector-open="false"]'), "Inspector must support a collapsed layout");
assert(css.includes('.app-shell[data-sidebar-open="false"]'), "Sidebar must support a collapsed layout");
assert(
  appSource.includes("data-inspector-resizing={inspectorResizing}") &&
    appSource.includes("onResizeStart={() => setInspectorResizing(true)}") &&
    css.includes('.app-shell[data-inspector-resizing="true"]'),
  "Inspector resizing must keep the panel grid and divider synchronized"
);
assert(
  css.includes(".app-shell::before") && css.includes(".app-shell::after"),
  "Pane dividers must extend through the titlebar"
);
assert(
  /\.thread-message \{[\s\S]*?width: min\(100%, 760px\);/.test(css) &&
    /\.composer-stack > \.composer \{[\s\S]*?width: min\(100%, 796px\);/.test(css),
  "Composer shell must remain inside the 760px conversation rail"
);
assert(
  /\.thread-output-artifacts \{[\s\S]*?display: grid;[\s\S]*?grid-template-columns: repeat\(auto-fill, minmax\(148px, 168px\)\);/.test(
    css
  ) &&
    /\.thread-output-image \{[\s\S]*?min-width: 0;[\s\S]*?overflow: hidden;/.test(css) &&
    /\.thread-output-link \{[\s\S]*?grid-column: 1 \/ -1;/.test(css),
  "Thread output previews must use isolated grid cells without overlap"
);
assert(
    appSource.includes('className="sidebar-resize-handle"') &&
    appSource.includes('aria-label="Resize sidebar"') &&
    css.includes("left: calc(var(--sidebar-layout-width) - 4px)") &&
    css.includes("rgba(0, 0, 0, 0.008) 68%") &&
    css.includes("rgba(0, 0, 0, 0.028) 100%") &&
    /\.sidebar-resize-handle::after \{[^}]*left: calc\(50% - 0\.5px\);[^}]*width: 0\.5px;[^}]*background: var\(--border-strong\);/.test(
      css
    ),
  "Sidebar divider must separate a crisp half-pixel edge from its subtle left-facing depth"
);
assert(
  css.includes(".window-workspace-header") &&
    css.includes("box-shadow: inset 0 -1px 0 var(--border)") &&
    !/\.window-workspace-header \{[^}]*left: var\(--sidebar-layout-width\)/.test(css),
  "Workspace metadata and its lower divider must live in the titlebar"
);
assert(!css.includes(".topbar {"), "Workspace must not retain a second header row");
assert(
    css.includes("--inspector-layout-width: min(var(--inspector-width, 320px), calc(100vw - 240px))") &&
    css.includes(".window-toolbar-panel-right") &&
    css.includes(".app-shell::after") &&
    css.includes("right: calc(var(--inspector-layout-width) - 1px)") &&
    css.includes("width: var(--inspector-layout-width)"),
  "Inspector and titlebar divider must share one bounded width"
);

const layouts = [
  { viewport: 1440, sidebar: 236, inspector: 320, overlay: false },
  { viewport: 1280, sidebar: 236, inspector: 320, overlay: false },
  { viewport: 1024, sidebar: 236, inspector: 320, overlay: true }
].map((layout) => ({
  ...layout,
  workspace: layout.viewport - layout.sidebar - (layout.overlay ? 0 : layout.inspector)
}));

for (const layout of layouts) {
  assert(
    layout.workspace >= 640,
    `${layout.viewport}px viewport leaves only ${layout.workspace}px for the workspace`
  );
}

console.log("desktop layout contracts ok");
for (const layout of layouts) {
  console.log(
    `${layout.viewport}px: workspace=${layout.workspace}px inspector=${layout.overlay ? "overlay" : `${layout.inspector}px`}`
  );
}
