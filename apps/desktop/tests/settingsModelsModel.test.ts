import assert from "node:assert/strict";
import test from "node:test";
import type { ProviderConfigInput } from "../src/tauri.ts";
import {
  applyFastModelToProfiles,
  configuredModelProfileCount,
  selectFastFallbackModel
} from "../src/providerModelAllocation.ts";

const draft = {
  providerId: "custom",
  providerResource: "",
  baseUrl: "https://gateway.test/v1",
  apiKey: "",
  model: "fast-model",
  conductorModel: "conductor-model",
  plannerModel: "reasoning-model",
  executorModel: "primary-model",
  reviewerModel: "verifier-model",
  summarizerModel: "utility-model",
  embeddingModel: "embedding-model",
  imageModel: "image-model",
  imageEndpoint: "",
  voiceModel: "voice-model",
  collaborationPolicy: "auto_router",
  promptEvolutionEnabled: false,
  contextWindowTokens: 128000,
  agentSystemPrompt: ""
} satisfies ProviderConfigInput;

test("selecting the Fast fallback does not overwrite service or profile slots", () => {
  const next = selectFastFallbackModel(draft, "new-fast-model", 200000);

  assert.equal(next.model, "new-fast-model");
  assert.equal(next.contextWindowTokens, 200000);
  assert.equal(next.conductorModel, draft.conductorModel);
  assert.equal(next.plannerModel, draft.plannerModel);
  assert.equal(next.executorModel, draft.executorModel);
  assert.equal(next.reviewerModel, draft.reviewerModel);
  assert.equal(next.summarizerModel, draft.summarizerModel);

  const withoutCatalogMetadata = selectFastFallbackModel(
    draft,
    "custom-fast-model",
    undefined
  );
  assert.equal(withoutCatalogMetadata.contextWindowTokens, draft.contextWindowTokens);
});

test("applying the Fast model to profiles is explicit and leaves the service override alone", () => {
  const next = applyFastModelToProfiles(draft);

  assert.equal(next.conductorModel, draft.conductorModel);
  assert.equal(next.plannerModel, draft.model);
  assert.equal(next.executorModel, draft.model);
  assert.equal(next.reviewerModel, draft.model);
  assert.equal(next.summarizerModel, draft.model);
});

test("profile allocation counts configured profile models, not services or fallbacks", () => {
  assert.equal(configuredModelProfileCount(draft), 4);
  assert.equal(
    configuredModelProfileCount({
      ...draft,
      plannerModel: " shared-model ",
      executorModel: "shared-model",
      reviewerModel: "",
      summarizerModel: "shared-model"
    }),
    1
  );
  assert.equal(configuredModelProfileCount(null), 0);
});
