import type { AgentEffort } from "./tauri";

export type WorkspaceView = "timeline" | "schedule" | "settings";

export const DEBUG_ALWAYS_VISIBLE_STORAGE_KEY = "cindx.debug.always-visible";
export const IGNORED_PERMISSION_REVIEWS_STORAGE_KEY = "cindx.permissions.ignored";
export const FOREGROUND_AGENT_POLL_INTERVAL_MS = 1_000;
export const BACKGROUND_AGENT_POLL_INTERVAL_MS = 5_000;

export function queuedMessageClientId() {
  const randomId =
    typeof globalThis.crypto !== "undefined" &&
    typeof globalThis.crypto.randomUUID === "function"
      ? globalThis.crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `agent-queue-client-${randomId}`;
}

export function loadDebugAlwaysVisible() {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(DEBUG_ALWAYS_VISIBLE_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

export function loadIgnoredPermissionReviewIds() {
  if (typeof window === "undefined") return new Set<string>();
  try {
    const values = JSON.parse(
      window.localStorage.getItem(IGNORED_PERMISSION_REVIEWS_STORAGE_KEY) ?? "[]"
    );
    return new Set<string>(
      Array.isArray(values) ? values.filter((value) => typeof value === "string") : []
    );
  } catch {
    return new Set<string>();
  }
}

export function normalizedSessionEffort(effort: string | undefined): AgentEffort {
  if (effort === "fast" || effort === "high" || effort === "xhigh") return effort;
  // Legacy tier labels migrate onto the reasoning levels.
  if (effort === "pro") return "high";
  return "default";
}

export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 320;
export const INSPECTOR_MIN_WIDTH = 280;
export const INSPECTOR_MAX_WIDTH = 420;
/**
 * The thread column must stay usable at the minimum window width even when
 * both panels are open at their user-chosen widths.
 */
export const MIN_THREAD_WIDTH = 240;

export function clampSidebarWidth(width: number) {
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width));
}

export function clampInspectorWidth(width: number) {
  return Math.min(INSPECTOR_MAX_WIDTH, Math.max(INSPECTOR_MIN_WIDTH, width));
}

/**
 * Window-aware panel constraint: the thread column keeps at least
 * MIN_THREAD_WIDTH. When the open panels plus the floor exceed the window,
 * the sidebar shrinks toward its minimum first, then the inspector; user
 * widths above the floor are never touched on wide windows.
 */
export function constrainPanelWidths(input: {
  windowWidth: number;
  sidebarOpen: boolean;
  sidebarWidth: number;
  inspectorOpen: boolean;
  inspectorWidth: number;
}): { sidebarWidth: number; inspectorWidth: number } {
  const sidebar = input.sidebarOpen ? clampSidebarWidth(input.sidebarWidth) : 0;
  const inspector = input.inspectorOpen ? clampInspectorWidth(input.inspectorWidth) : 0;
  let overflow = sidebar + inspector + MIN_THREAD_WIDTH - input.windowWidth;
  if (overflow <= 0) return { sidebarWidth: sidebar, inspectorWidth: inspector };
  const sidebarCut = Math.max(0, Math.min(overflow, sidebar - SIDEBAR_MIN_WIDTH));
  const constrainedSidebar = sidebar - sidebarCut;
  overflow -= sidebarCut;
  const inspectorCut = Math.max(0, Math.min(overflow, inspector - INSPECTOR_MIN_WIDTH));
  return {
    sidebarWidth: constrainedSidebar,
    inspectorWidth: inspector - inspectorCut
  };
}
