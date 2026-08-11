import type { ProviderConfigInput } from "./tauri";

export function selectFastFallbackModel(
  draft: ProviderConfigInput,
  model: string,
  contextWindowTokens: number | null | undefined
): ProviderConfigInput {
  return {
    ...draft,
    model,
    contextWindowTokens: contextWindowTokens ?? draft.contextWindowTokens
  };
}

export function applyFastModelToProfiles(
  draft: ProviderConfigInput
): ProviderConfigInput {
  return {
    ...draft,
    plannerModel: draft.model,
    executorModel: draft.model,
    reviewerModel: draft.model,
    summarizerModel: draft.model
  };
}

export function configuredModelProfileCount(
  draft: ProviderConfigInput | null
): number {
  if (!draft) return 0;
  return new Set(
    [
      draft.executorModel,
      draft.plannerModel,
      draft.reviewerModel,
      draft.summarizerModel
    ]
      .map((model) => model.trim())
      .filter(Boolean)
  ).size;
}
