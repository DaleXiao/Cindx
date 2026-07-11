import fs from "node:fs";

const css = fs.readFileSync("apps/desktop/src/styles.css", "utf8");
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
assert(css.includes("grid-template-columns: 236px minmax(0, 1fr) var(--inspector-layout-width)"), "Wide layout must use three panes");
assert(
  css.includes("--titlebar-height: 46px") &&
    css.includes("grid-template-rows: var(--titlebar-height) minmax(0, 1fr)"),
  "Layout must reserve a custom titlebar row"
);
assert(css.includes("@media (max-width: 1180px)"), "Compact desktop breakpoint is missing");
assert(css.includes('position: fixed;\n    z-index: 20;'), "Compact inspector must become an overlay");
assert(css.includes('.app-shell[data-inspector-open="false"]'), "Inspector must support a collapsed layout");
assert(css.includes('.app-shell[data-sidebar-open="false"]'), "Sidebar must support a collapsed layout");
assert(
  css.includes(".window-toolbar::before") && css.includes(".app-shell::after"),
  "Pane dividers must extend through the titlebar"
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
  { viewport: 1024, sidebar: 220, inspector: 320, overlay: true }
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
