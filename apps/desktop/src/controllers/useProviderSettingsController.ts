import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getPhase4State,
  listProviderModels,
  saveProviderConfig,
  setPromptEvolutionEnabled,
  validateImageEndpoint,
  type Phase4State,
  type ProviderConfigInput,
  type ProviderConfigState
} from "../tauri";

function normalizedEffortPolicy(policy: string) {
  if (policy === "single" || policy === "best_of_n") return policy;
  return "auto_router";
}

function providerDraftFromState(provider: ProviderConfigState): ProviderConfigInput {
  return {
    baseUrl: provider.baseUrl,
    apiKey: "",
    model: provider.model,
    conductorModel: provider.conductorModel,
    plannerModel: provider.plannerModel,
    executorModel: provider.executorModel,
    reviewerModel: provider.reviewerModel,
    summarizerModel: provider.summarizerModel,
    embeddingModel: provider.embeddingModel,
    imageModel: provider.imageModel,
    imageEndpoint: provider.imageEndpoint,
    collaborationPolicy: normalizedEffortPolicy(provider.collaborationPolicy),
    promptEvolutionEnabled: provider.promptEvolutionEnabled,
    contextWindowTokens: provider.contextWindowTokens,
    agentSystemPrompt: provider.agentSystemPrompt
  };
}

type ProviderSettingsControllerOptions = {
  reportError: (message: string | null) => void;
  showSaved: (message?: string) => void;
};

export function useProviderSettingsController({
  reportError,
  showSaved
}: ProviderSettingsControllerOptions) {
  const [phase4, setPhase4] = useState<Phase4State | null>(null);
  const [providerDraft, setProviderDraft] = useState<ProviderConfigInput | null>(null);
  const [providerBusy, setProviderBusy] = useState(false);
  const [providerModels, setProviderModels] = useState<string[]>([]);
  const [providerModelsBusy, setProviderModelsBusy] = useState(false);
  const [providerModelsRefreshTurn, setProviderModelsRefreshTurn] = useState(0);
  const [providerModelsError, setProviderModelsError] = useState<string | null>(null);
  const [imageEndpointValidation, setImageEndpointValidation] = useState<
    "idle" | "checking" | "valid" | "invalid"
  >("idle");
  const imageEndpointValidationRequestRef = useRef(0);

  const loadProviderState = useCallback(async () => {
    const state = await getPhase4State();
    setPhase4(state);
    setProviderDraft(providerDraftFromState(state.provider));
    reportError(state.lastError);
    return state;
  }, [reportError]);

  useEffect(() => {
    const requestId = imageEndpointValidationRequestRef.current + 1;
    imageEndpointValidationRequestRef.current = requestId;
    const imageEndpoint = providerDraft?.imageEndpoint.trim() ?? "";
    const imageModel = providerDraft?.imageModel.trim() ?? "";
    if (!providerDraft || !imageEndpoint || !imageModel) {
      setImageEndpointValidation("idle");
      return;
    }
    try {
      const parsed = new URL(imageEndpoint);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error("unsupported URL");
    } catch {
      setImageEndpointValidation("invalid");
      return;
    }

    setImageEndpointValidation("checking");
    const timer = window.setTimeout(() => {
      void validateImageEndpoint({
        baseUrl: providerDraft.baseUrl,
        imageModel: providerDraft.imageModel,
        imageEndpoint: providerDraft.imageEndpoint
      })
        .then((result) => {
          if (imageEndpointValidationRequestRef.current !== requestId) return;
          setImageEndpointValidation(result.valid ? "valid" : "invalid");
        })
        .catch(() => {
          if (imageEndpointValidationRequestRef.current === requestId) {
            setImageEndpointValidation("invalid");
          }
        });
    }, 600);
    return () => window.clearTimeout(timer);
  }, [providerDraft?.baseUrl, providerDraft?.imageEndpoint, providerDraft?.imageModel]);

  const providerModelOptions = useMemo(() => {
    const configured = providerDraft
      ? [
          providerDraft.model,
          providerDraft.conductorModel,
          providerDraft.plannerModel,
          providerDraft.executorModel,
          providerDraft.reviewerModel,
          providerDraft.summarizerModel,
          providerDraft.embeddingModel,
          providerDraft.imageModel
        ]
      : [];
    return [...new Set([...providerModels, ...configured].filter(Boolean))].sort();
  }, [providerDraft, providerModels]);

  const collaborationModelCount = providerDraft
    ? new Set([
        providerDraft.plannerModel,
        providerDraft.executorModel,
        providerDraft.reviewerModel,
        providerDraft.summarizerModel
      ]).size
    : 0;

  const handleSaveProviderConfig = useCallback(async () => {
    if (!providerDraft) return;
    setProviderBusy(true);
    reportError(null);
    try {
      const next = await saveProviderConfig(providerDraft);
      setPhase4(next);
      setProviderDraft(providerDraftFromState(next.provider));
      showSaved();
    } finally {
      setProviderBusy(false);
    }
  }, [providerDraft, reportError, showSaved]);

  const handlePromptEvolutionToggle = useCallback(
    async (enabled: boolean) => {
      if (!providerDraft || providerBusy) return;
      const previous = providerDraft.promptEvolutionEnabled;
      setProviderDraft({ ...providerDraft, promptEvolutionEnabled: enabled });
      setProviderBusy(true);
      try {
        const next = await setPromptEvolutionEnabled(enabled);
        setPhase4(next);
        setProviderDraft(providerDraftFromState(next.provider));
        showSaved(enabled ? "Prompt evolution enabled" : "Prompt evolution disabled");
      } catch (error) {
        setProviderDraft({ ...providerDraft, promptEvolutionEnabled: previous });
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setProviderBusy(false);
      }
    },
    [providerBusy, providerDraft, reportError, showSaved]
  );

  const handleLoadProviderModels = useCallback(async () => {
    if (!providerDraft || providerModelsBusy) return;
    setProviderModelsRefreshTurn((current) => current + 1);
    setProviderModelsBusy(true);
    setProviderModelsError(null);
    try {
      const next = await listProviderModels({
        baseUrl: providerDraft.baseUrl,
        apiKey: providerDraft.apiKey
      });
      setProviderModels(next.models);
      setProviderModelsError(next.lastError);
    } finally {
      setProviderModelsBusy(false);
    }
  }, [providerDraft, providerModelsBusy]);

  return {
    collaborationModelCount,
    handleLoadProviderModels,
    handlePromptEvolutionToggle,
    handleSaveProviderConfig,
    imageEndpointValidation,
    loadProviderState,
    phase4,
    providerBusy,
    providerDraft,
    providerModelOptions,
    providerModels,
    providerModelsBusy,
    providerModelsError,
    providerModelsRefreshTurn,
    setProviderDraft
  };
}
