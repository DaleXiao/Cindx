import { RefreshCw } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { ProviderConfigInput } from "../tauri";
import {
  PROVIDER_OPTIONS,
  providerBaseUrl,
  providerPresetModelGroups,
  selectProviderDraft,
  type ProviderId
} from "../providerProfiles";

type ProviderConnectionFieldsProps = {
  canUseConfiguredKey: boolean;
  handleLoadProviderModels: () => Promise<void>;
  providerDraft: ProviderConfigInput;
  providerBusy: boolean;
  providerModels: string[];
  providerModelsBusy: boolean;
  providerModelsError: string | null;
  providerModelsRefreshTurn: number;
  refreshAnimationClass?: string;
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function ProviderConnectionFields({
  canUseConfiguredKey,
  handleLoadProviderModels,
  providerDraft,
  providerBusy,
  providerModels,
  providerModelsBusy,
  providerModelsError,
  providerModelsRefreshTurn,
  refreshAnimationClass,
  setProviderDraft
}: ProviderConnectionFieldsProps) {
  const resolvedBaseUrl = providerBaseUrl(
    providerDraft.providerId,
    providerDraft.providerResource,
    providerDraft.baseUrl
  );
  const presetModels = providerPresetModelGroups(providerDraft.providerId);
  const presetModelCount = new Set(Object.values(presetModels).flat()).size;
  const showManualCatalogRefresh = providerDraft.providerId === "custom";

  return (
    <>
      <label>
        <span>Model provider</span>
        <select
          disabled={providerBusy}
          value={providerDraft.providerId}
          onChange={(event) =>
            setProviderDraft((current) => {
              if (!current) return current;
              const providerId = event.target.value as ProviderId;
              return selectProviderDraft(current, providerId);
            })
          }
        >
          {PROVIDER_OPTIONS.map((provider) => (
            <option value={provider.id} key={provider.id}>
              {provider.label}
            </option>
          ))}
        </select>
      </label>
      {providerDraft.providerId === "azure_openai" && (
        <label>
          <span>Azure resource name</span>
          <input
            disabled={providerBusy}
            value={providerDraft.providerResource}
            placeholder="my-openai-resource"
            spellCheck={false}
            onChange={(event) => {
              const providerResource = event.target.value;
              setProviderDraft((current) =>
                current ? { ...current, providerResource } : current
              );
            }}
          />
        </label>
      )}
      {providerDraft.providerId === "custom" && (
        <label>
          <span>Base URL</span>
          <input
            disabled={providerBusy}
            value={providerDraft.baseUrl}
            placeholder="https://provider.example/v1"
            spellCheck={false}
            onChange={(event) => {
              const baseUrl = event.target.value;
              setProviderDraft((current) => (current ? { ...current, baseUrl } : current));
            }}
          />
        </label>
      )}
      {providerDraft.providerId !== "custom" && (
        <p className="provider-auto-note">
          {providerDraft.providerId === "azure_openai"
            ? "Azure exception: its API key does not contain the resource or deployment names, so those two values are required; service endpoints are derived automatically."
            : providerDraft.providerId === "alibaba_cn"
              ? `The standard DashScope Pay-as-you-go endpoint and ${presetModelCount} model presets are built in; no Workspace is required.`
              : `Endpoints and ${presetModelCount} model presets across supported modalities are built in.`}
        </p>
      )}
      <label>
        <span>
          {providerDraft.providerId === "alibaba_cn"
            ? "DashScope API key (Pay-as-you-go)"
            : "API key"}
        </span>
        <input
          disabled={providerBusy}
          type="password"
          value={providerDraft.apiKey}
          autoComplete="off"
          spellCheck={false}
          placeholder={canUseConfiguredKey ? "Configured key" : "Enter API key"}
          onChange={(event) => {
            const apiKey = event.target.value;
            setProviderDraft((current) => (current ? { ...current, apiKey } : current));
          }}
        />
      </label>
      <div className="model-catalog-row">
        {showManualCatalogRefresh && (
          <button
            className="secondary-button"
            type="button"
            disabled={
              providerModelsBusy ||
              providerBusy ||
              !resolvedBaseUrl ||
              (!providerDraft.apiKey.trim() && !canUseConfiguredKey)
            }
            onClick={() => void handleLoadProviderModels()}
          >
            <RefreshCw
              aria-hidden="true"
              className={refreshAnimationClass}
              key={providerModelsRefreshTurn}
            />
            <span>{providerModelsBusy ? "Loading models" : "Load models"}</span>
          </button>
        )}
        <span>
          {providerDraft.providerId === "azure_openai"
            ? "Enter Azure deployment names below"
            : providerDraft.providerId !== "custom"
              ? "Connecting verifies the key and selected Chat model; modality presets are applied automatically"
            : providerModels.length > 0
              ? `${providerModels.length} available`
              : "You can also enter model IDs manually"}
        </span>
      </div>
      {providerModelsError && (
        <div className="settings-inline-error" role="alert" aria-live="polite">
          {providerModelsError}
        </div>
      )}
    </>
  );
}
