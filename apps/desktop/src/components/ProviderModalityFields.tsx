import { CheckCircle2 } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { ProviderConfigInput } from "../tauri";
import {
  providerModelCatalogHasCapability,
  providerVoiceTransport,
  type ProviderModelGroups
} from "../providerProfiles";
import { ProviderModelInput as ModelSelect } from "./ProviderModelInput";

type ProviderModalityFieldsProps = {
  imageEndpointValidation: "idle" | "checking" | "valid" | "invalid";
  providerBusy: boolean;
  providerDraft: ProviderConfigInput;
  providerModelOptions: ProviderModelGroups;
  providerModels: string[];
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function ProviderModalityFields({
  imageEndpointValidation,
  providerBusy,
  providerDraft,
  providerModelOptions,
  providerModels,
  setProviderDraft
}: ProviderModalityFieldsProps) {
  const voiceSupported = providerVoiceTransport(
    providerDraft.providerId,
    providerDraft.baseUrl
  ) !== "none";
  const voiceModelAvailable = Boolean(
    voiceSupported &&
      providerModelCatalogHasCapability(
        providerDraft.providerId,
        providerModels,
        "voice",
        providerDraft.voiceModel
      )
  );
  const customImageEndpointReady =
    providerDraft.providerId !== "custom" ||
    !providerDraft.imageEndpoint.trim() ||
    imageEndpointValidation === "valid";
  const imageModelAvailable = Boolean(
    customImageEndpointReady &&
      providerModelCatalogHasCapability(
        providerDraft.providerId,
        providerModels,
        "image",
        providerDraft.imageModel
      )
  );

  return (
    <>
      <div className="role-grid provider-meta-grid">
        <ModelSelect
          label="Embedding"
          value={providerDraft.embeddingModel}
          options={providerModelOptions.embedding}
          disabled={providerBusy}
          status={{
            available: providerModels.includes(providerDraft.embeddingModel),
            label: "Embedding model available through the latest authenticated provider connection",
            title: "The selected embedding model is present in the latest authenticated model catalog"
          }}
          onChange={(embeddingModel) => setProviderDraft({ ...providerDraft, embeddingModel })}
        />
        <ModelSelect
          label="Speech recognition"
          value={providerDraft.voiceModel}
          options={providerModelOptions.voice}
          emptyLabel={voiceSupported ? "Not configured" : "Not supported by this adapter"}
          disabled={providerBusy || !voiceSupported}
          status={{
            available: voiceModelAvailable,
            label:
              "Speech recognition model available through the latest authenticated provider connection",
            title:
              "The selected speech recognition model is present in the latest authenticated model catalog and its ASR transport is available"
          }}
          onChange={(voiceModel) => setProviderDraft({ ...providerDraft, voiceModel })}
        />
        <ModelSelect
          label="Image generation"
          value={providerDraft.imageModel}
          options={providerModelOptions.image}
          disabled={providerBusy}
          emptyLabel="Not configured"
          status={{
            available: imageModelAvailable,
            label:
              "Image generation model available through the latest authenticated provider connection",
            title:
              providerDraft.providerId === "custom" && providerDraft.imageEndpoint.trim()
                ? "The selected image model is present in the latest authenticated model catalog and the custom image endpoint is reachable"
                : "The selected image model is present in the latest authenticated model catalog"
          }}
          onChange={(imageModel) => setProviderDraft({ ...providerDraft, imageModel })}
        />
        {providerDraft.providerId === "custom" && (
          <label>
            <span>Image API endpoint (optional)</span>
            <div className="provider-endpoint-input" data-validation={imageEndpointValidation}>
              <input
                disabled={providerBusy}
                value={providerDraft.imageEndpoint}
                spellCheck={false}
                placeholder="Uses the custom Base URL when empty"
                onChange={(event) => {
                  const imageEndpoint = event.target.value;
                  setProviderDraft((current) =>
                    current ? { ...current, imageEndpoint } : current
                  );
                }}
              />
              {imageEndpointValidation === "valid" && (
                <CheckCircle2
                  className="provider-endpoint-check"
                  aria-label="Image endpoint reachable"
                />
              )}
            </div>
          </label>
        )}
      </div>
      {!voiceSupported && (
        <p className="provider-auto-note">
          Speech recognition is disabled because this provider needs a different ASR transport
          adapter.
        </p>
      )}
    </>
  );
}
