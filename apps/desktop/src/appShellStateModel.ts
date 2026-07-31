import type { InspectorTab } from "./components/Inspector";
import type { SettingsCategory } from "./components/SettingsPage";
import type { WorkspaceView } from "./appShellModel";

export type InspectorOutputRequest = {
  sessionId: string;
  path: string;
  nonce: number;
};

export type AppShellState = {
  activeView: WorkspaceView;
  selectedScheduleId: string | null;
  workspaceViewBeforeSettings: Exclude<WorkspaceView, "settings">;
  sidebarOpen: boolean;
  settingsCategory: SettingsCategory;
  inspectorTab: InspectorTab;
  inspectorOpen: boolean;
  inspectorOutputRequest: InspectorOutputRequest | null;
  inspectorOpenBeforeSettings: boolean;
  inspectorOpenBeforeSchedule: boolean;
  inspectorWidth: number;
  inspectorResizing: boolean;
  debugAlwaysVisible: boolean;
};

type StateUpdate<Value> = Value | ((previous: Value) => Value);
type FieldAction = {
  [Key in keyof AppShellState]: {
    type: "set_field";
    key: Key;
    value: StateUpdate<AppShellState[Key]>;
  };
}[keyof AppShellState];

export type AppShellAction =
  | FieldAction
  | { type: "change_view"; view: WorkspaceView }
  | { type: "open_settings"; category: SettingsCategory }
  | { type: "show_inspector"; tab: InspectorTab }
  | { type: "show_output"; request: InspectorOutputRequest };

export function createInitialAppShellState(debugAlwaysVisible = false): AppShellState {
  return {
    activeView: "timeline",
    selectedScheduleId: null,
    workspaceViewBeforeSettings: "timeline",
    sidebarOpen: true,
    settingsCategory: "runtime",
    inspectorTab: "details",
    inspectorOpen: false,
    inspectorOutputRequest: null,
    inspectorOpenBeforeSettings: false,
    inspectorOpenBeforeSchedule: false,
    inspectorWidth: 320,
    inspectorResizing: false,
    debugAlwaysVisible
  };
}

function finishViewTransition(state: AppShellState, next: AppShellState): AppShellState {
  return state.activeView === next.activeView &&
    state.workspaceViewBeforeSettings === next.workspaceViewBeforeSettings &&
    state.inspectorOpen === next.inspectorOpen &&
    state.inspectorOpenBeforeSettings === next.inspectorOpenBeforeSettings &&
    state.inspectorOpenBeforeSchedule === next.inspectorOpenBeforeSchedule
    ? state
    : next;
}

function showWorkspaceView(
  state: AppShellState,
  view: Exclude<WorkspaceView, "settings">
): AppShellState {
  const leavingSettings = state.activeView === "settings";
  const leavingSchedule = state.activeView === "schedule" && view === "timeline";
  const enteringSchedule = state.activeView !== "schedule" && view === "schedule";
  const inspectorOpenBeforeSchedule =
    enteringSchedule && state.activeView === "timeline"
      ? state.inspectorOpen
      : state.inspectorOpenBeforeSchedule;
  const inspectorOpen = enteringSchedule
    ? false
    : leavingSettings && view === "timeline"
      ? state.inspectorOpenBeforeSettings
      : leavingSchedule
        ? inspectorOpenBeforeSchedule
        : state.inspectorOpen;
  return finishViewTransition(state, {
    ...state,
    activeView: view,
    workspaceViewBeforeSettings: view,
    inspectorOpen,
    inspectorOpenBeforeSchedule
  });
}

export function transitionAppShellView(
  state: AppShellState,
  view: WorkspaceView
): AppShellState {
  if (view !== "settings") return showWorkspaceView(state, view);
  if (state.activeView === "settings") {
    return showWorkspaceView(state, state.workspaceViewBeforeSettings);
  }
  return {
    ...state,
    activeView: "settings",
    workspaceViewBeforeSettings: state.activeView,
    inspectorOpenBeforeSettings:
      state.activeView === "schedule"
        ? state.inspectorOpenBeforeSchedule
        : state.inspectorOpen,
    inspectorOpen: false
  };
}

export function appShellReducer(state: AppShellState, action: AppShellAction): AppShellState {
  if (action.type === "change_view") return transitionAppShellView(state, action.view);
  if (action.type === "open_settings") {
    const next = state.activeView === "settings" ? state : transitionAppShellView(state, "settings");
    return next.settingsCategory === action.category
      ? next
      : { ...next, settingsCategory: action.category };
  }
  if (action.type === "show_inspector") {
    return state.inspectorOpen && state.inspectorTab === action.tab
      ? state
      : { ...state, inspectorOpen: true, inspectorTab: action.tab };
  }
  if (action.type === "show_output") {
    return { ...state, inspectorOpen: true, inspectorOutputRequest: action.request };
  }
  const current = state[action.key];
  const value =
    typeof action.value === "function"
      ? (action.value as (previous: typeof current) => typeof current)(current)
      : action.value;
  return Object.is(current, value) ? state : { ...state, [action.key]: value };
}
