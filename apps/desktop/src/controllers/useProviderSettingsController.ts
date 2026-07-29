import { useCallback, useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
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
import {
  bindProviderDraftApiKey,
  groupProviderModels,
  providerBaseUrl,
  providerCanUseConfiguredKey,
  providerModelCatalogIdentity,
  providerSupportsWebRtcVoice
} from "../providerProfiles";

function normalizedEffortPolicy(policy: string) {
  if (policy === "single" || policy === "best_of_n") return policy;
  return "auto_router";
}

function providerDraftFromState(provider: ProviderConfigState): ProviderConfigInput {
  return {
    providerId: provider.providerId,
    providerResource: provider.providerResource,
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
    voiceModel: provider.voiceModel,
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
  const [providerDraft, setProviderDraftState] = useState<ProviderConfigInput | null>(null);
  const setProviderDraft = useCallback(
    (update: SetStateAction<ProviderConfigInput | null>) => {
      setProviderDraftState((current) => {
        const next = typeof update === "function" ? update(current) : update;
        if (!current || !next) return next;
        return bindProviderDraftApiKey(current, next);
      });
    },
    []
  );
  const [providerBusy, setProviderBusy] = useState(false);
  const [providerModels, setProviderModels] = useState<string[]>([]);
  const [providerModelsBusy, setProviderModelsBusy] = useState(false);
  const [providerModelsRefreshTurn, setProviderModelsRefreshTurn] = useState(0);
  const [providerModelsError, setProviderModelsError] = useState<string | null>(null);
  const [imageEndpointValidation, setImageEndpointValidation] = useState<
    "idle" | "checking" | "valid" | "invalid"
  >("idle");
  const imageEndpointValidationRequestRef = useRef(0);
  const modelCatalogRequestRef = useRef(0);

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
    if (
      !providerDraft ||
      providerDraft.providerId !== "custom" ||
      !imageEndpoint ||
      !imageModel
    ) {
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
        providerId: providerDraft.providerId,
        providerResource: providerDraft.providerResource,
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
  }, [
    providerDraft?.providerId,
    providerDraft?.providerResource,
    providerDraft?.baseUrl,
    providerDraft?.imageEndpoint,
    providerDraft?.imageModel
  ]);

  const providerModelOptions = useMemo(() => {
    if (!providerDraft) {
      return { chat: [], embedding: [], image: [], voice: [] };
    }
    return groupProviderModels(providerModels, {
      chat: [
        providerDraft.model,
        providerDraft.conductorModel,
        providerDraft.plannerModel,
        providerDraft.executorModel,
        providerDraft.reviewerModel,
        providerDraft.summarizerModel
      ],
      embedding: providerDraft.embeddingModel,
      image: providerDraft.imageModel,
      voice: providerDraft.voiceModel
    });
  }, [providerDraft, providerModels]);

  const providerCatalogIdentity = providerDraft
    ? providerModelCatalogIdentity(providerDraft)
    : "";

  useEffect(() => {
    modelCatalogRequestRef.current += 1;
    setProviderModels([]);
    setProviderModelsBusy(false);
    setProviderModelsError(null);
  }, [providerCatalogIdentity]);

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
    const requestId = modelCatalogRequestRef.current + 1;
    modelCatalogRequestRef.current = requestId;
    setProviderModelsRefreshTurn((current) => current + 1);
    setProviderModelsBusy(true);
    setProviderModelsError(null);
    try {
      const next = await listProviderModels({
        providerId: providerDraft.providerId,
        providerResource: providerDraft.providerResource,
        baseUrl: providerBaseUrl(
          providerDraft.providerId,
          providerDraft.providerResource,
          providerDraft.baseUrl
        ),
        apiKey: providerDraft.apiKey
      });
      if (modelCatalogRequestRef.current !== requestId) return;
      setProviderModels(next.models);
      setProviderModelsError(next.lastError);
    } catch (error) {
      if (modelCatalogRequestRef.current !== requestId) return;
      const message = error instanceof Error ? error.message : String(error);
      setProviderModelsError(message);
      reportError(message);
    } finally {
      if (modelCatalogRequestRef.current === requestId) setProviderModelsBusy(false);
    }
  }, [providerDraft, providerModelsBusy, reportError]);

  const savedProvider = phase4?.provider ?? null;
  const canUseConfiguredKey = Boolean(
    savedProvider?.apiKeySet &&
      providerDraft &&
      providerCanUseConfiguredKey(providerDraft, savedProvider)
  );

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
    canUseConfiguredKey,
    setProviderDraft,
    voiceConfigured: Boolean(
      phase4?.provider.apiKeySet &&
        phase4.provider.baseUrl.trim() &&
        phase4.provider.voiceModel.trim() &&
        providerSupportsWebRtcVoice(phase4.provider.providerId, phase4.provider.baseUrl)
    )
  };
}
