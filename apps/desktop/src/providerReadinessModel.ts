export type ProviderReadiness = "checking" | "ready" | "unconfigured" | "error";

type SavedChatProvider = {
  ready: boolean;
};

export type ProviderSubmissionPreflight = {
  allowSubmit: boolean;
  clearDraft: boolean;
  showModelsCta: boolean;
};

export function resolveProviderReadiness(
  provider: SavedChatProvider | null,
  runtimeReady: boolean | null,
  loadFailed: boolean
): ProviderReadiness {
  if (provider) return provider.ready ? "ready" : "unconfigured";
  if (runtimeReady !== null) return runtimeReady ? "ready" : "unconfigured";
  return loadFailed ? "error" : "checking";
}

export function providerSubmissionPreflight(
  readiness: ProviderReadiness
): ProviderSubmissionPreflight {
  if (readiness === "ready") {
    return { allowSubmit: true, clearDraft: true, showModelsCta: false };
  }
  return {
    allowSubmit: false,
    clearDraft: false,
    showModelsCta: readiness !== "checking"
  };
}

export function providerReadinessMessage(readiness: ProviderReadiness) {
  if (readiness === "checking") {
    return "Checking model provider configuration. Your draft is unchanged.";
  }
  if (readiness === "error") {
    return "Provider settings could not be loaded. Open Models to retry.";
  }
  return "Connect a Chat model in Settings before sending.";
}

export function providerStatusText(readiness: ProviderReadiness) {
  if (readiness === "ready") return "Ready";
  if (readiness === "unconfigured") return "Unconfigured";
  return readiness === "error" ? "Provider unavailable" : "Checking provider";
}
