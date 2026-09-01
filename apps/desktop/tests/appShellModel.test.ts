import assert from "node:assert/strict";
import test from "node:test";
import {
  clampInspectorWidth,
  clampSidebarWidth,
  constrainPanelWidths,
  INSPECTOR_MAX_WIDTH,
  INSPECTOR_MIN_WIDTH,
  MIN_THREAD_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH
} from "../src/appShellModel.ts";

test("panel width clamps stay inside their documented ranges", () => {
  assert.equal(clampSidebarWidth(100), SIDEBAR_MIN_WIDTH);
  assert.equal(clampSidebarWidth(900), SIDEBAR_MAX_WIDTH);
  assert.equal(clampInspectorWidth(100), INSPECTOR_MIN_WIDTH);
  assert.equal(clampInspectorWidth(900), INSPECTOR_MAX_WIDTH);
});

test("wide windows keep user panel widths untouched", () => {
  const layout = constrainPanelWidths({
    windowWidth: 1440,
    sidebarOpen: true,
    sidebarWidth: SIDEBAR_MAX_WIDTH,
    inspectorOpen: true,
    inspectorWidth: INSPECTOR_MAX_WIDTH
  });
  assert.equal(layout.sidebarWidth, SIDEBAR_MAX_WIDTH);
  assert.equal(layout.inspectorWidth, INSPECTOR_MAX_WIDTH);
});

test("the minimum window keeps the thread column usable with both panels open", () => {
  const layout = constrainPanelWidths({
    windowWidth: 960,
    sidebarOpen: true,
    sidebarWidth: SIDEBAR_MAX_WIDTH,
    inspectorOpen: true,
    inspectorWidth: INSPECTOR_MAX_WIDTH
  });
  const thread = 960 - layout.sidebarWidth - layout.inspectorWidth;
  assert.ok(
    thread >= MIN_THREAD_WIDTH,
    `thread must keep ${MIN_THREAD_WIDTH}px, got ${thread}px`
  );
  assert.ok(layout.sidebarWidth >= SIDEBAR_MIN_WIDTH);
  assert.ok(layout.inspectorWidth >= INSPECTOR_MIN_WIDTH);
});

test("the sidebar yields before the inspector when space runs out", () => {
  const layout = constrainPanelWidths({
    windowWidth: 970,
    sidebarOpen: true,
    sidebarWidth: SIDEBAR_MAX_WIDTH,
    inspectorOpen: true,
    inspectorWidth: INSPECTOR_MAX_WIDTH
  });
  // 320 + 420 + 240 = 980 > 970, so exactly 10px must be reclaimed.
  assert.equal(layout.sidebarWidth, SIDEBAR_MAX_WIDTH - 10);
  assert.equal(layout.inspectorWidth, INSPECTOR_MAX_WIDTH);
});

test("extremely narrow windows exhaust sidebar slack before touching the inspector", () => {
  const layout = constrainPanelWidths({
    windowWidth: 800,
    sidebarOpen: true,
    sidebarWidth: SIDEBAR_MAX_WIDTH,
    inspectorOpen: true,
    inspectorWidth: INSPECTOR_MAX_WIDTH
  });
  assert.equal(layout.sidebarWidth, SIDEBAR_MIN_WIDTH);
  assert.equal(layout.inspectorWidth, INSPECTOR_MAX_WIDTH - 60);
  assert.equal(800 - layout.sidebarWidth - layout.inspectorWidth, MIN_THREAD_WIDTH);
});

test("closed panels contribute no width regardless of stored sizes", () => {
  const layout = constrainPanelWidths({
    windowWidth: 960,
    sidebarOpen: false,
    sidebarWidth: SIDEBAR_MAX_WIDTH,
    inspectorOpen: false,
    inspectorWidth: INSPECTOR_MAX_WIDTH
  });
  assert.equal(layout.sidebarWidth, 0);
  assert.equal(layout.inspectorWidth, 0);
});
