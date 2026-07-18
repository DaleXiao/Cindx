import fs from "node:fs";

const css = fs.readFileSync("apps/desktop/src/styles.css", "utf8");
const appSource = fs.readFileSync("apps/desktop/src/App.tsx", "utf8");
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
  /@media \(max-width: 1180px\)[\s\S]*?\.window-workspace-header \{[\s\S]*?right: var\(--inspector-layout-width\);/.test(
    css
  ),
  "Compact titlebar must preserve the inspector surface above the overlay"
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
    appSource.includes('className="sidebar-resize-handle"') &&
    appSource.includes('aria-label="Resize sidebar"') &&
    css.includes("left: calc(var(--sidebar-layout-width) - 7px)") &&
    css.includes("width: 7px") &&
    css.includes("rgba(0, 0, 0, 0.012) 58%") &&
    css.includes("rgba(0, 0, 0, 0.07) 100%"),
  "Sidebar divider must expose a clean single-layer left-facing depth"
);
assert(
  css.includes(".window-workspace-header") &&
    css.includes("box-shadow: inset 0 -1px 0 var(--border)"),
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
