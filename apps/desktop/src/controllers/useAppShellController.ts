import {
  useCallback,
  useMemo,
  useReducer,
  type Dispatch,
  type SetStateAction
} from "react";
import type { InspectorTab } from "../components/Inspector";
import type { SettingsCategory } from "../components/SettingsPage";
import { loadDebugAlwaysVisible, type WorkspaceView } from "../appShellModel";
import {
  appShellReducer,
  createInitialAppShellState,
  type AppShellAction,
  type AppShellState,
  type InspectorOutputRequest
} from "../appShellStateModel";

function fieldSetter<Key extends keyof AppShellState>(
  dispatch: Dispatch<AppShellAction>,
  key: Key
): Dispatch<SetStateAction<AppShellState[Key]>> {
  return (value) => dispatch({ type: "set_field", key, value } as AppShellAction);
}

export function useAppShellController() {
  const [state, dispatch] = useReducer(
    appShellReducer,
    undefined,
    () => createInitialAppShellState(loadDebugAlwaysVisible())
  );
  const setters = useMemo(
    () => ({
      setSelectedScheduleId: fieldSetter(dispatch, "selectedScheduleId"),
      setBootstrapFailed: fieldSetter(dispatch, "bootstrapFailed"),
      setSidebarOpen: fieldSetter(dispatch, "sidebarOpen"),
      setSettingsCategory: fieldSetter(dispatch, "settingsCategory"),
      setInspectorTab: fieldSetter(dispatch, "inspectorTab"),
      setInspectorOpen: fieldSetter(dispatch, "inspectorOpen"),
      setInspectorWidth: fieldSetter(dispatch, "inspectorWidth"),
      setInspectorResizing: fieldSetter(dispatch, "inspectorResizing"),
      setDebugAlwaysVisible: fieldSetter(dispatch, "debugAlwaysVisible")
    }),
    []
  );
  const handleWorkspaceViewChange = useCallback(
    (view: WorkspaceView) => dispatch({ type: "change_view", view }),
    []
  );
  const showTimelineView = useCallback(
    () => dispatch({ type: "change_view", view: "timeline" }),
    []
  );
  const openSettingsCategory = useCallback(
    (category: SettingsCategory) => dispatch({ type: "open_settings", category }),
    []
  );
  const showInspector = useCallback(
    (tab: InspectorTab) => dispatch({ type: "show_inspector", tab }),
    []
  );
  const showInspectorOutput = useCallback(
    (request: InspectorOutputRequest) => dispatch({ type: "show_output", request }),
    []
  );
  return {
    ...state,
    ...setters,
    handleWorkspaceViewChange,
    openSettingsCategory,
    showInspector,
    showInspectorOutput,
    showTimelineView
  };
}
