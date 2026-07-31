import assert from "node:assert/strict";
import test from "node:test";
import {
  appShellReducer,
  createInitialAppShellState,
  transitionAppShellView
} from "../src/appShellStateModel.ts";

test("settings restores the timeline inspector and toggles back to its prior view", () => {
  const timeline = { ...createInitialAppShellState(), inspectorOpen: true };
  const settings = transitionAppShellView(timeline, "settings");
  assert.equal(settings.activeView, "settings");
  assert.equal(settings.inspectorOpen, false);
  assert.equal(settings.inspectorOpenBeforeSettings, true);
  assert.equal(transitionAppShellView(settings, "settings").inspectorOpen, true);
});

test("schedule and nested settings preserve the timeline inspector snapshot", () => {
  const timeline = { ...createInitialAppShellState(), inspectorOpen: true };
  const schedule = transitionAppShellView(timeline, "schedule");
  assert.equal(schedule.inspectorOpen, false);
  assert.equal(schedule.inspectorOpenBeforeSchedule, true);

  const settings = transitionAppShellView(schedule, "settings");
  const restoredSchedule = transitionAppShellView(settings, "settings");
  assert.equal(restoredSchedule.activeView, "schedule");
  assert.equal(restoredSchedule.inspectorOpen, false);
  assert.equal(transitionAppShellView(restoredSchedule, "timeline").inspectorOpen, true);
});

test("unchanged navigation keeps state identity and shell actions update atomically", () => {
  const initial = createInitialAppShellState();
  assert.equal(transitionAppShellView(initial, "timeline"), initial);

  const models = appShellReducer(initial, { type: "open_settings", category: "models" });
  assert.equal(models.activeView, "settings");
  assert.equal(models.settingsCategory, "models");
  const inspector = appShellReducer(models, { type: "show_inspector", tab: "trace" });
  assert.equal(inspector.inspectorOpen, true);
  assert.equal(inspector.inspectorTab, "trace");

  const request = { sessionId: "session-1", path: "artifact.txt", nonce: 1 };
  const output = appShellReducer(models, { type: "show_output", request });
  assert.equal(output.inspectorOpen, true);
  assert.equal(output.inspectorOutputRequest, request);
});

test("shell field updates preserve React functional-setter semantics", () => {
  const initial = createInitialAppShellState();
  const closed = appShellReducer(initial, {
    type: "set_field",
    key: "sidebarOpen",
    value: (open) => !open
  });
  assert.equal(closed.sidebarOpen, false);
  assert.equal(
    appShellReducer(closed, { type: "set_field", key: "sidebarOpen", value: false }),
    closed
  );
});
