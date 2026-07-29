import assert from "node:assert/strict";
import test from "node:test";
import {
  ALIBABA_CN_BASE_URL,
  ALIBABA_CN_IMAGE_ENDPOINT,
  OPENAI_BASE_URL,
  PROVIDER_OPTIONS,
  bindProviderDraftApiKey,
  groupProviderModels,
  isValidProviderBaseUrl,
  providerApiKeySetAfterSave,
  providerBaseUrl,
  providerCanUseConfiguredKey,
  providerModelCatalogIdentity,
  providerModelContextWindow,
  providerPresetModelGroups,
  providerSupportsWebRtcVoice,
  resolveProviderProfile,
  selectProviderDraft
} from "../src/providerProfiles.ts";

test("provider presets resolve fixed and resource-scoped endpoints", () => {
  assert.equal(providerBaseUrl("openai", "", "https://ignored.test"), OPENAI_BASE_URL);
  assert.equal(
    providerBaseUrl("alibaba_cn", "", "https://ignored.test"),
    ALIBABA_CN_BASE_URL
  );
  assert.equal(
    providerBaseUrl("alibaba_cn", "workspace-1", ""),
    ALIBABA_CN_BASE_URL
  );
  assert.equal(
    providerBaseUrl("azure_openai", "My-Resource", ""),
    "https://my-resource.openai.azure.com/openai/v1"
  );
  assert.equal(
    providerBaseUrl(
      "azure_openai",
      "My-Resource",
      "https://my-resource.services.ai.azure.com/openai/v1"
    ),
    "https://my-resource.services.ai.azure.com/openai/v1"
  );
  assert.equal(providerBaseUrl("azure_openai", "invalid.resource", ""), "");
  assert.equal(
    providerBaseUrl("azure_openai", "a".repeat(63), ""),
    `https://${"a".repeat(63)}.openai.azure.com/openai/v1`
  );
  assert.equal(providerBaseUrl("azure_openai", "a".repeat(64), ""), "");
  assert.equal(providerBaseUrl("custom", "", "https://gateway.test/v1"), "https://gateway.test/v1");
  assert.equal(isValidProviderBaseUrl("https://gateway.test/v1"), true);
  assert.equal(isValidProviderBaseUrl("not-a-url"), false);
  assert.equal(PROVIDER_OPTIONS.at(-1)?.id, "custom");
});

test("saved API keys are reusable only for the same provider identity", () => {
  const saved = { providerId: "openai" as const, providerResource: "", baseUrl: OPENAI_BASE_URL };
  assert.equal(providerCanUseConfiguredKey(saved, saved), true);
  assert.equal(
    providerCanUseConfiguredKey(
      { providerId: "alibaba_cn", providerResource: "", baseUrl: ALIBABA_CN_BASE_URL },
      saved
    ),
    false
  );
  assert.equal(
    providerCanUseConfiguredKey(
      { providerId: "custom", providerResource: "", baseUrl: "https://one.test/v1" },
      { providerId: "custom", providerResource: "", baseUrl: "https://two.test/v1" }
    ),
    false
  );
  assert.equal(
    providerCanUseConfiguredKey(
      {
        providerId: "custom",
        providerResource: "",
        baseUrl: "https://one.test/v1",
        imageEndpoint: "https://images.test/v1"
      },
      { providerId: "custom", providerResource: "", baseUrl: "https://one.test/v1" }
    ),
    false
  );
});

test("typed API keys are cleared whenever the draft provider identity changes", () => {
  const custom = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://one.test/v1",
    imageEndpoint: "https://images.test/generate",
    apiKey: "typed-secret"
  };

  assert.equal(
    bindProviderDraftApiKey(custom, { ...custom, baseUrl: "https://two.test/v1" }).apiKey,
    ""
  );
  assert.equal(
    bindProviderDraftApiKey(custom, {
      ...custom,
      imageEndpoint: "https://other-images.test/generate"
    }).apiKey,
    ""
  );
  assert.equal(
    bindProviderDraftApiKey(custom, {
      ...custom,
      baseUrl: "https://one.test/compatible-mode/v1"
    }).apiKey,
    "typed-secret"
  );

  const azure = {
    ...custom,
    providerId: "azure_openai" as const,
    providerResource: "resource-one"
  };
  assert.equal(
    bindProviderDraftApiKey(azure, { ...azure, providerResource: "resource-two" }).apiKey,
    ""
  );
});

test("provider presets keep unsaved custom endpoints hidden for later restoration", () => {
  const custom = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://gateway.test/v1",
    imageEndpoint: "https://images.test/generate",
    apiKey: "custom-secret"
  };

  const openai = selectProviderDraft(custom, "openai");
  assert.equal(openai.baseUrl, custom.baseUrl);
  assert.equal(openai.imageEndpoint, custom.imageEndpoint);
  assert.equal(openai.apiKey, "");

  const restored = selectProviderDraft(openai, "custom");
  assert.equal(restored.baseUrl, custom.baseUrl);
  assert.equal(restored.imageEndpoint, custom.imageEndpoint);
});

test("selecting a provider atomically applies every built-in modality default", () => {
  const current = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://gateway.test/v1",
    imageEndpoint: "https://images.test/generate",
    apiKey: "custom-secret",
    model: "custom-chat",
    conductorModel: "custom-chat",
    plannerModel: "custom-chat",
    executorModel: "custom-chat",
    reviewerModel: "custom-chat",
    summarizerModel: "custom-chat",
    embeddingModel: "custom-embedding",
    imageModel: "custom-image",
    voiceModel: "custom-realtime",
    contextWindowTokens: 32768
  };

  const openai = selectProviderDraft(current, "openai");
  assert.equal(openai.apiKey, "");
  assert.equal(openai.model, "gpt-4.1");
  assert.equal(openai.summarizerModel, "gpt-4.1-mini");
  assert.equal(openai.embeddingModel, "text-embedding-3-large");
  assert.equal(openai.imageModel, "gpt-image-2");
  assert.equal(openai.voiceModel, "gpt-realtime-2.1");
  assert.equal(openai.contextWindowTokens, 1047576);

  const alibaba = selectProviderDraft(openai, "alibaba_cn");
  assert.equal(alibaba.providerResource, "");
  assert.equal(alibaba.model, "qwen3.7-plus");
  assert.equal(alibaba.summarizerModel, "qwen3.7-flash");
  assert.equal(alibaba.embeddingModel, "text-embedding-v4");
  assert.equal(alibaba.imageModel, "qwen-image-3.0-pro");
  assert.equal(alibaba.voiceModel, "");
  assert.equal(alibaba.contextWindowTokens, 1000000);
});

test("browser fallback saves canonical provider endpoints and isolates saved keys", () => {
  const savedCustom = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://gateway.test/v1",
    imageEndpoint: "https://images.test/generate",
    apiKeySet: true
  };
  const openaiInput = {
    ...savedCustom,
    providerId: "openai" as const,
    baseUrl: savedCustom.baseUrl,
    imageEndpoint: savedCustom.imageEndpoint,
    apiKey: ""
  };
  const openaiProfile = resolveProviderProfile(openaiInput);

  assert.equal(openaiProfile.baseUrl, OPENAI_BASE_URL);
  assert.equal(openaiProfile.imageEndpoint, "");
  assert.equal(
    providerApiKeySetAfterSave({ ...openaiInput, ...openaiProfile }, savedCustom),
    false
  );
  assert.equal(
    providerApiKeySetAfterSave(
      { ...openaiInput, ...openaiProfile },
      { ...openaiProfile, apiKeySet: true }
    ),
    true
  );

  const alibaba = resolveProviderProfile({
    providerId: "alibaba_cn",
    providerResource: "",
    baseUrl: "https://ignored.test",
    imageEndpoint: "https://ignored-images.test"
  });
  assert.equal(alibaba.baseUrl, ALIBABA_CN_BASE_URL);
  assert.equal(alibaba.imageEndpoint, ALIBABA_CN_IMAGE_ENDPOINT);
  assert.equal(alibaba.providerResource, "");
});

test("model catalog identity invalidates same-origin path and API key changes", () => {
  const first = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://gateway.test/v1",
    imageEndpoint: "",
    apiKey: "catalog-key-one"
  };
  const nextPath = { ...first, baseUrl: "https://gateway.test/compatible/v1" };
  const nextKey = { ...first, apiKey: "catalog-key-two" };

  assert.equal(providerCanUseConfiguredKey(first, nextPath), true);
  assert.notEqual(providerModelCatalogIdentity(first), providerModelCatalogIdentity(nextPath));
  assert.notEqual(providerModelCatalogIdentity(first), providerModelCatalogIdentity(nextKey));
});

test("model catalogs are separated by capability without dropping configured values", () => {
  const groups = groupProviderModels(
    "custom",
    [
      "gpt-4.1-mini",
      "gpt-4o",
      "text-embedding-3-small",
      "gpt-image-1",
      "gpt-realtime",
      "whisper-1"
    ],
    {
      chat: ["private-chat-deployment"],
      embedding: "private-embedding-deployment",
      image: "private-image-deployment",
      voice: "private-voice-deployment"
    }
  );

  assert.deepEqual(groups.chat, ["gpt-4.1-mini", "gpt-4o", "private-chat-deployment"]);
  assert.deepEqual(groups.multimodal, ["gpt-4.1-mini", "gpt-4o"]);
  assert.deepEqual(groups.embedding, ["private-embedding-deployment", "text-embedding-3-small"]);
  assert.deepEqual(groups.image, ["gpt-image-1", "private-image-deployment"]);
  assert.deepEqual(groups.voice, ["gpt-realtime", "private-voice-deployment"]);
  assert.equal(providerSupportsWebRtcVoice("alibaba_cn", ALIBABA_CN_BASE_URL), false);
  assert.equal(providerSupportsWebRtcVoice("openai", OPENAI_BASE_URL), true);
});

test("built-in catalogs explicitly separate multimodal and generation capabilities", () => {
  const openai = providerPresetModelGroups("openai");
  assert.deepEqual(openai.multimodal, ["gpt-4.1", "gpt-4.1-mini", "gpt-4o"]);
  assert.deepEqual(openai.image, ["gpt-image-2", "gpt-image-1.5", "gpt-image-1"]);
  assert.deepEqual(openai.voice, ["gpt-realtime-2.1", "gpt-realtime-2.1-mini", "gpt-realtime"]);

  const alibaba = providerPresetModelGroups("alibaba_cn");
  assert.equal(alibaba.multimodal.includes("qwen3.7-plus"), true);
  assert.deepEqual(alibaba.embedding, ["text-embedding-v4"]);
  assert.deepEqual(alibaba.image, ["qwen-image-3.0-pro"]);
  assert.deepEqual(alibaba.voice, []);
  assert.equal(providerModelContextWindow("openai", "gpt-4.1"), 1047576);
  assert.equal(providerModelContextWindow("alibaba_cn", "qwen3.7-plus"), 1000000);
});

test("custom providers disable WebRTC only for exact known incompatible hosts", () => {
  for (const baseUrl of [
    "https://team.openai.azure.com/openai/v1",
    "https://team.services.ai.azure.com/openai/v1",
    ALIBABA_CN_BASE_URL,
    "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
  ]) {
    assert.equal(providerSupportsWebRtcVoice("custom", baseUrl), false, baseUrl);
  }

  for (const baseUrl of [
    "https://gateway.example.test/v1",
    "https://team.openai.azure.com.evil.test/v1",
    "https://team.services.ai.azure.com.evil.test/v1",
    "https://dashscope.aliyuncs.com.evil.test/v1",
    "https://workspace.cn-beijing.maas.aliyuncs.com.evil.test/v1"
  ]) {
    assert.equal(providerSupportsWebRtcVoice("custom", baseUrl), true, baseUrl);
  }
});
