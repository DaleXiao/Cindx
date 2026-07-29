export type ProviderId = "openai" | "azure_openai" | "alibaba_cn" | "custom";

export const PROVIDER_OPTIONS: ReadonlyArray<{ id: ProviderId; label: string }> = [
  { id: "openai", label: "OpenAI" },
  { id: "azure_openai", label: "Azure OpenAI" },
  { id: "alibaba_cn", label: "阿里云（中国）" },
  { id: "custom", label: "自定义 URL" }
];

export const OPENAI_BASE_URL = "https://api.openai.com/v1";
export const ALIBABA_CN_BASE_URL = "https://dashscope.aliyuncs.com/compatible-mode/v1";
export const ALIBABA_CN_IMAGE_ENDPOINT =
  "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";

export type ProviderModelGroups = {
  chat: string[];
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
  if (providerId === "alibaba_cn" || providerId === "azure_openai") {
    const resource = providerResource.trim().toLowerCase();
    if (providerId === "alibaba_cn" && !resource) return ALIBABA_CN_BASE_URL;
    if (!validResourceLabel(resource)) return "";
    return providerId === "azure_openai"
      ? `https://${resource}.openai.azure.com/openai/v1`
      : `https://${resource}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1`;
  }
  return customBaseUrl.trim();
}

export function providerIdentity(draft: ProviderDraftIdentity) {
  const baseUrl = providerBaseUrl(draft.providerId, draft.providerResource, draft.baseUrl);
  if (draft.providerId === "azure_openai" || draft.providerId === "alibaba_cn") {
    return `${draft.providerId}:${draft.providerResource.trim().toLowerCase()}`;
  }
  if (draft.providerId !== "custom") return draft.providerId;
  const imageEndpoint = draft.imageEndpoint?.trim() || baseUrl;
  return `${draft.providerId}:${endpointOrigin(baseUrl)}:${endpointOrigin(imageEndpoint)}`;
}

export function resolveProviderProfile(draft: ProviderDraftIdentity) {
  const providerResource =
    draft.providerId === "azure_openai" || draft.providerId === "alibaba_cn"
      ? draft.providerResource.trim().toLowerCase()
      : "";
  const baseUrl = providerBaseUrl(draft.providerId, providerResource, draft.baseUrl);
  let imageEndpoint = "";
  if (draft.providerId === "custom") {
    imageEndpoint = draft.imageEndpoint?.trim() ?? "";
  } else if (draft.providerId === "alibaba_cn" && baseUrl) {
    imageEndpoint = providerResource
      ? `https://${providerResource}.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation`
      : ALIBABA_CN_IMAGE_ENDPOINT;
  }
  return {
    providerId: draft.providerId,
    providerResource,
    baseUrl,
    imageEndpoint
  };
}

export function providerModelCatalogIdentity(
  draft: ProviderDraftIdentity & { apiKey?: string }
) {
  const profile = resolveProviderProfile(draft);
  return JSON.stringify([
    profile.providerId,
    profile.providerResource,
    profile.baseUrl,
    draft.apiKey ?? ""
  ]);
}

export function providerCanUseConfiguredKey(
  draft: ProviderDraftIdentity,
  saved: ProviderDraftIdentity | null
) {
  return saved !== null && providerIdentity(draft) === providerIdentity(saved);
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
  return bindProviderDraftApiKey(current, {
    ...current,
    providerId,
    providerResource: ""
  });
}

export function providerSupportsWebRtcVoice(providerId: ProviderId, baseUrl: string) {
  if (providerId === "openai") return true;
  if (providerId !== "custom") return false;
  const host = endpointHost(baseUrl);
  if (host === "dashscope.aliyuncs.com") return false;
  if (hostHasResourceSuffix(host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX)) return false;
  return !AZURE_OPENAI_HOST_SUFFIXES.some((suffix) => hostHasResourceSuffix(host, suffix));
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
  if (id.includes("realtime")) return "voice";
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
    id.includes("transcribe") ||
    id.includes("whisper") ||
    id.includes("speech") ||
    id.includes("text-to-speech") ||
    id.includes("tts") ||
    id.includes("asr") ||
    id.includes("rerank")
  ) {
    return "other";
  }
  return "chat";
}

export function groupProviderModels(
  models: string[],
  configured: {
    chat: string[];
    embedding: string;
    image: string;
    voice: string;
  }
): ProviderModelGroups {
  const groups: ProviderModelGroups = {
    chat: [...configured.chat],
    embedding: [configured.embedding],
    image: [configured.image],
    voice: [configured.voice]
  };
  for (const model of models) {
    const kind = modelKind(model);
    if (kind !== "other") groups[kind].push(model);
  }
  return {
    chat: uniqueSorted(groups.chat),
    embedding: uniqueSorted(groups.embedding),
    image: uniqueSorted(groups.image),
    voice: uniqueSorted(groups.voice)
  };
}
