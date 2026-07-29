import { RefreshCw } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { ProviderConfigInput } from "../tauri";
import {
  PROVIDER_OPTIONS,
  providerBaseUrl,
  selectProviderDraft,
  type ProviderId
} from "../providerProfiles";

type ProviderConnectionFieldsProps = {
  canUseConfiguredKey: boolean;
  handleLoadProviderModels: () => Promise<void>;
  providerDraft: ProviderConfigInput;
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
  const providerCatalogSupported = providerDraft.providerId !== "azure_openai";

  return (
    <>
      <label>
        <span>Model provider</span>
        <select
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
      {providerDraft.providerId === "alibaba_cn" && (
        <label>
          <span>Workspace ID (optional)</span>
          <input
            value={providerDraft.providerResource}
            placeholder="Uses the shared China endpoint when empty"
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
          Chat, multimodal, embedding, and image endpoints are configured automatically.
        </p>
      )}
      <label>
        <span>API key</span>
        <input
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
        {providerCatalogSupported && (
          <button
            className="secondary-button"
            type="button"
            disabled={
              providerModelsBusy ||
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
          {!providerCatalogSupported
            ? "Enter Azure deployment names below"
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
