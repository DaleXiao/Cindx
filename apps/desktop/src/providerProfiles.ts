import providerCatalog from "../../../crates/model-provider/providerCatalog.json" with { type: "json" };

export type ProviderId = "openai" | "azure_openai" | "alibaba_cn" | "custom";
export type ProviderVoiceTransport = "openai_webrtc" | "dashscope_websocket" | "none";

type ProviderPreset = (typeof providerCatalog.providers)[number];

const PROVIDER_PRESETS = new Map(
  providerCatalog.providers.map((provider) => [provider.id as ProviderId, provider])
);

export const PROVIDER_OPTIONS: ReadonlyArray<{ id: ProviderId; label: string }> =
  providerCatalog.providers.map((provider) => ({
    id: provider.id as ProviderId,
    label: provider.label
  }));

export const OPENAI_BASE_URL = PROVIDER_PRESETS.get("openai")?.baseUrl ?? "";
export const ALIBABA_CN_BASE_URL = PROVIDER_PRESETS.get("alibaba_cn")?.baseUrl ?? "";
export const ALIBABA_CN_IMAGE_ENDPOINT =
  PROVIDER_PRESETS.get("alibaba_cn")?.imageEndpoint ?? "";

export type ProviderModelGroups = {
  chat: string[];
  multimodal: string[];
  embedding: string[];
  image: string[];
  voice: string[];
};

const RESOURCE_LABEL_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;
const AZURE_OPENAI_HOST_SUFFIXES = [".openai.azure.com", ".services.ai.azure.com"] as const;
const ALIBABA_CN_WORKSPACE_HOST_SUFFIX = ".cn-beijing.maas.aliyuncs.com";

type ProviderDraftIdentity = {
  providerId: ProviderId;
  providerResource: string;
  baseUrl: string;
  imageEndpoint?: string;
};

type ProviderCredentialDraft = ProviderDraftIdentity & {
  apiKey: string;
};

type SavedProviderCredential = ProviderDraftIdentity & {
  apiKeySet: boolean;
};

type ProviderModelDraft = {
  model: string;
  conductorModel: string;
  plannerModel: string;
  executorModel: string;
  reviewerModel: string;
  summarizerModel: string;
  embeddingModel: string;
  imageModel: string;
  voiceModel: string;
  contextWindowTokens: number;
};

export function providerPreset(providerId: ProviderId): ProviderPreset | null {
  return PROVIDER_PRESETS.get(providerId) ?? null;
}

export function providerSupportsModelDiscovery(providerId: ProviderId) {
  return Boolean(providerPreset(providerId)?.modelDiscovery);
}

export type ProviderTierDefaults = {
  fast: string;
  auto: string;
  pro: string;
};

export function providerTierDefaults(providerId: ProviderId): ProviderTierDefaults {
  const defaults = providerPreset(providerId)?.defaults;
  return {
    fast: defaults?.fast ?? "",
    auto: defaults?.auto ?? "",
    pro: defaults?.pro ?? ""
  };
}

export function providerPresetModelGroups(providerId: ProviderId): ProviderModelGroups {
  const models = providerPreset(providerId)?.models ?? [];
  const withModality = (modality: string) =>
    models.filter((model) => model.modalities.includes(modality)).map((model) => model.id);
  return {
    chat: withModality("chat"),
    multimodal: withModality("imageInput"),
    embedding: withModality("embedding"),
    image: withModality("imageGeneration"),
    voice: [...withModality("speechRecognition"), ...withModality("realtime")]
  };
}

export function providerModelContextWindow(providerId: ProviderId, modelId: string) {
  return providerPreset(providerId)?.models.find(
    (model) => model.id.toLowerCase() === modelId.trim().toLowerCase()
  )?.contextWindowTokens;
}

/** Short display name for a catalog model id: strips date-style suffixes
 * (e.g. "-0731", "-0813") and trailing -preview/-latest so the composer capsule
 * reads the model name instead of the long id. */
export function modelDisplayName(modelId: string): string {
  const trimmed = modelId.trim();
  if (!trimmed) return trimmed;
  return trimmed
    .replace(/-\d{4}$/, "")
    .replace(/-(preview|latest|turbo|alpha|beta)$/i, "");
}

function endpointOrigin(value: string) {
  try {
    return new URL(value).origin.toLowerCase();
  } catch {
    return value.toLowerCase();
  }
}

function validResourceLabel(value: string) {
  return value.length > 0 && value.length <= 63 && RESOURCE_LABEL_PATTERN.test(value);
}

function endpointHost(value: string) {
  try {
    return new URL(value).hostname.toLowerCase().replace(/\.+$/, "");
  } catch {
    return "";
  }
}

function hostHasResourceSuffix(host: string, suffix: string) {
  return host.endsWith(suffix) && validResourceLabel(host.slice(0, -suffix.length));
}

export function providerBaseUrl(
  providerId: ProviderId,
  providerResource: string,
  customBaseUrl: string
) {
  if (providerId === "openai") return OPENAI_BASE_URL;
  if (providerId === "alibaba_cn") return ALIBABA_CN_BASE_URL;
  if (providerId === "azure_openai") {
    const resource = providerResource.trim().toLowerCase();
    if (!validResourceLabel(resource)) return "";
    const configuredHost = endpointHost(customBaseUrl);
    if (
      configuredHost === `${resource}.openai.azure.com` ||
      configuredHost === `${resource}.services.ai.azure.com`
    ) {
      try {
        const configured = new URL(customBaseUrl.trim());
        if (configured.protocol === "https:") return customBaseUrl.trim().replace(/\/+$/, "");
      } catch {
        // Fall through to the canonical Azure OpenAI host.
      }
    }
    return `https://${resource}.openai.azure.com/openai/v1`;
  }
  return customBaseUrl.trim();
}

export function isValidProviderBaseUrl(value: string) {
  try {
    const url = new URL(value.trim());
    return (url.protocol === "http:" || url.protocol === "https:") && Boolean(url.hostname);
  } catch {
    return false;
  }
}

export function providerIdentity(draft: ProviderDraftIdentity) {
  const baseUrl = providerBaseUrl(draft.providerId, draft.providerResource, draft.baseUrl);
  if (draft.providerId === "azure_openai") {
    return `${draft.providerId}:${draft.providerResource.trim().toLowerCase()}`;
  }
  if (draft.providerId !== "custom") return draft.providerId;
  const imageEndpoint = draft.imageEndpoint?.trim() || baseUrl;
  return `${draft.providerId}:${endpointOrigin(baseUrl)}:${endpointOrigin(imageEndpoint)}`;
}

export function resolveProviderProfile(draft: ProviderDraftIdentity) {
  const providerResource =
    draft.providerId === "azure_openai" ? draft.providerResource.trim().toLowerCase() : "";
  const baseUrl = providerBaseUrl(draft.providerId, providerResource, draft.baseUrl);
  let imageEndpoint = "";
  if (draft.providerId === "custom") {
    imageEndpoint = draft.imageEndpoint?.trim() ?? "";
  } else {
    imageEndpoint = providerPreset(draft.providerId)?.imageEndpoint ?? "";
  }
  return {
    providerId: draft.providerId,
    providerResource,
    baseUrl,
    imageEndpoint
  };
}

export function providerModelCatalogIdentity(draft: ProviderDraftIdentity) {
  const profile = resolveProviderProfile(draft);
  return JSON.stringify([
    profile.providerId,
    profile.providerResource,
    profile.baseUrl,
    profile.imageEndpoint
  ]);
}

export function providerCanUseConfiguredKey(
  draft: ProviderDraftIdentity,
  saved: ProviderDraftIdentity | null
) {
  return saved !== null && providerIdentity(draft) === providerIdentity(saved);
}

export function providerModelCatalogMatchesDraft(
  catalogIdentity: string | null,
  catalogApiKey: string | null,
  draft: ProviderCredentialDraft
) {
  return (
    catalogIdentity === providerModelCatalogIdentity(draft) &&
    catalogApiKey === draft.apiKey
  );
}

export function providerModelCatalogApiKeyAfterSave(
  catalogApiKey: string | null,
  submittedApiKey: string
) {
  return catalogApiKey === submittedApiKey ? "" : null;
}

export function providerApiKeySetAfterSave(
  draft: ProviderCredentialDraft,
  saved: SavedProviderCredential | null
) {
  return Boolean(draft.apiKey.trim()) || Boolean(saved?.apiKeySet && providerCanUseConfiguredKey(draft, saved));
}

export function bindProviderDraftApiKey<T extends ProviderCredentialDraft>(
  previous: T,
  next: T
): T {
  if (providerIdentity(previous) === providerIdentity(next)) return next;
  return { ...next, apiKey: "" };
}

export function selectProviderDraft<T extends ProviderCredentialDraft>(
  current: T,
  providerId: ProviderId
): T {
  const next = {
    ...current,
    providerId,
    providerResource: ""
  } as T;
  const preset = providerPreset(providerId);
  if (preset && "model" in current) {
    const defaults = preset.defaults;
    Object.assign(next, {
      model: defaults.chat,
      conductorModel: defaults.conductor,
      plannerModel: defaults.planner,
      executorModel: defaults.executor,
      reviewerModel: defaults.reviewer,
      summarizerModel: defaults.summarizer,
      embeddingModel: defaults.embedding,
      imageModel: defaults.image,
      voiceModel: defaults.voice,
      contextWindowTokens: defaults.contextWindowTokens
    } satisfies ProviderModelDraft);
  }
  return bindProviderDraftApiKey(current, next);
}

export function providerSupportsWebRtcVoice(providerId: ProviderId, baseUrl: string) {
  if (providerId !== "custom") return Boolean(providerPreset(providerId)?.webRtcVoice);
  const host = endpointHost(baseUrl);
  if (host === "dashscope.aliyuncs.com") return false;
  if (hostHasResourceSuffix(host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX)) return false;
  return !AZURE_OPENAI_HOST_SUFFIXES.some((suffix) => hostHasResourceSuffix(host, suffix));
}

export function providerVoiceTransport(
  providerId: ProviderId,
  baseUrl: string
): ProviderVoiceTransport {
  if (providerId === "alibaba_cn") return "dashscope_websocket";
  return providerSupportsWebRtcVoice(providerId, baseUrl) ? "openai_webrtc" : "none";
}

function uniqueSorted(values: string[]) {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))].sort((left, right) =>
    left.localeCompare(right)
  );
}

function modelKind(model: string): "chat" | "embedding" | "image" | "voice" | "other" {
  const id = model.trim().toLowerCase();
  if (!id) return "other";
  if (id.includes("embedding") || id.startsWith("gte-")) return "embedding";
  if (
    id.includes("asr") ||
    id.includes("transcribe") ||
    id.includes("whisper") ||
    id.includes("speech-recognition") ||
    id.includes("realtime")
  ) {
    return "voice";
  }
  if (
    id.includes("gpt-image") ||
    id.includes("dall-e") ||
    id.includes("qwen-image") ||
    id.includes("wanx") ||
    id.includes("text-to-image") ||
    id.includes("t2i") ||
    id.includes("stable-diffusion") ||
    id.startsWith("flux")
  ) {
    return "image";
  }
  if (
    id.includes("speech") ||
    id.includes("text-to-speech") ||
    id.includes("tts") ||
    id.includes("rerank")
  ) {
    return "other";
  }
  return "chat";
}

function modelLooksMultimodal(model: string) {
  const id = model.trim().toLowerCase().replace(/_/g, "-");
  return (
    id.includes("vision") ||
    id.includes("-vl") ||
    id.includes("omni") ||
    id.includes("pixtral") ||
    id.includes("llava") ||
    id.includes("glm-4v") ||
    id.startsWith("gpt-4o") ||
    id.startsWith("gpt-4.1") ||
    id.startsWith("gpt-5") ||
    id.startsWith("gemini") ||
    id.startsWith("claude-3") ||
    id.startsWith("claude-4")
  );
}

export function groupProviderModels(
  providerId: ProviderId,
  models: string[],
  configured: {
    chat: string[];
    embedding: string;
    image: string;
    voice: string;
  }
): ProviderModelGroups {
  const preset = providerPresetModelGroups(providerId);
  const groups: ProviderModelGroups = {
    chat: [...preset.chat, ...configured.chat],
    multimodal: [...preset.multimodal],
    embedding: [...preset.embedding, configured.embedding],
    image: [...preset.image, configured.image],
    voice: [...preset.voice, configured.voice]
  };
  for (const model of models) {
    const kind = modelKind(model);
    if (kind !== "other") groups[kind].push(model);
    if (providerId === "custom" && kind === "chat" && modelLooksMultimodal(model)) {
      groups.multimodal.push(model);
    }
  }
  if (providerId === "custom") {
    for (const model of configured.chat) {
      if (modelLooksMultimodal(model)) groups.multimodal.push(model);
    }
  }
  return {
    chat: uniqueSorted(groups.chat),
    multimodal: uniqueSorted(groups.multimodal),
    embedding: uniqueSorted(groups.embedding),
    image: uniqueSorted(groups.image),
    voice: uniqueSorted(groups.voice)
  };
}

export function providerModelCatalogHasCapability(
  providerId: ProviderId,
  models: string[],
  capability: "image" | "voice",
  selectedModel: string
) {
  const normalizedModel = selectedModel.trim().toLocaleLowerCase();
  if (
    !normalizedModel ||
    !models.some((model) => model.trim().toLocaleLowerCase() === normalizedModel)
  ) {
    return false;
  }
  const groups = groupProviderModels(providerId, models, {
    chat: [],
    embedding: "",
    image: "",
    voice: ""
  });
  return groups[capability].some(
    (model) => model.trim().toLocaleLowerCase() === normalizedModel
  );
}
