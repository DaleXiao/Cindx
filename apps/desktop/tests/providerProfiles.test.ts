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
  providerModelCatalogApiKeyAfterSave,
  providerModelCatalogHasCapability,
  providerModelCatalogIdentity,
  providerModelCatalogMatchesDraft,
  providerModelContextWindow,
  providerPresetModelGroups,
  providerSupportsModelDiscovery,
  providerSupportsWebRtcVoice,
  providerVoiceTransport,
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

test("model refresh visibility follows each provider discovery capability", () => {
  assert.equal(providerSupportsModelDiscovery("openai"), true);
  assert.equal(providerSupportsModelDiscovery("alibaba_cn"), true);
  assert.equal(providerSupportsModelDiscovery("custom"), true);
  assert.equal(providerSupportsModelDiscovery("azure_openai"), false);
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
  assert.equal(alibaba.voiceModel, "qwen3-asr-flash-realtime");
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

test("model catalog identity survives saved-key masking but invalidates endpoint changes", () => {
  const first = {
    providerId: "custom" as const,
    providerResource: "",
    baseUrl: "https://gateway.test/v1",
    imageEndpoint: "",
    apiKey: "catalog-key-one"
  };
  const nextPath = { ...first, baseUrl: "https://gateway.test/compatible/v1" };
  const nextImageEndpoint = {
    ...first,
    imageEndpoint: "https://gateway.test/images/generations"
  };
  const nextKey = { ...first, apiKey: "catalog-key-two" };
  const maskedAfterConnect = { ...first, apiKey: "" };

  assert.equal(providerCanUseConfiguredKey(first, nextPath), true);
  assert.notEqual(providerModelCatalogIdentity(first), providerModelCatalogIdentity(nextPath));
  assert.notEqual(
    providerModelCatalogIdentity(first),
    providerModelCatalogIdentity(nextImageEndpoint)
  );
  assert.equal(providerModelCatalogIdentity(first), providerModelCatalogIdentity(nextKey));
  assert.equal(providerModelCatalogIdentity(first), providerModelCatalogIdentity(maskedAfterConnect));
  assert.equal(
    providerModelCatalogMatchesDraft(
      providerModelCatalogIdentity(first),
      first.apiKey,
      first
    ),
    true
  );
  assert.equal(
    providerModelCatalogMatchesDraft(
      providerModelCatalogIdentity(first),
      first.apiKey,
      nextKey
    ),
    false
  );
  assert.equal(
    providerModelCatalogApiKeyAfterSave(first.apiKey, first.apiKey),
    maskedAfterConnect.apiKey
  );
  assert.equal(providerModelCatalogApiKeyAfterSave("old-key", first.apiKey), null);
});

test("modality availability requires the selected model in the refreshed capability catalog", () => {
  const models = ["gpt-4.1", "gpt-image-2", "gpt-realtime-2.1"];
  assert.equal(
    providerModelCatalogHasCapability("openai", models, "image", "gpt-image-2"),
    true
  );
  assert.equal(
    providerModelCatalogHasCapability("openai", models, "voice", "gpt-realtime-2.1"),
    true
  );
  assert.equal(
    providerModelCatalogHasCapability("openai", models, "voice", "gpt-image-2"),
    false
  );
  assert.equal(
    providerModelCatalogHasCapability("openai", models, "image", "gpt-image-1"),
    false
  );
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
  assert.deepEqual(groups.voice, ["gpt-realtime", "private-voice-deployment", "whisper-1"]);
  assert.equal(providerSupportsWebRtcVoice("alibaba_cn", ALIBABA_CN_BASE_URL), false);
  assert.equal(providerSupportsWebRtcVoice("openai", OPENAI_BASE_URL), true);
  assert.equal(providerVoiceTransport("alibaba_cn", ALIBABA_CN_BASE_URL), "dashscope_websocket");
  assert.equal(providerVoiceTransport("openai", OPENAI_BASE_URL), "openai_webrtc");
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
  assert.deepEqual(alibaba.voice, [
    "qwen3-asr-flash-realtime",
    "fun-asr-realtime",
    "qwen3.5-omni-flash-realtime"
  ]);
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
