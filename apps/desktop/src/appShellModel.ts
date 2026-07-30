import type { AgentEffort } from "./tauri";

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
  if (effort === "fast" || effort === "pro") return effort;
  return "auto";
}

export function clampSidebarWidth(width: number) {
  return Math.min(320, Math.max(200, width));
}
