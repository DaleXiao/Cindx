import { KeyRound, RefreshCw, Save } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { Phase4State, ProviderConfigInput } from "../tauri";
import {
  isValidProviderBaseUrl,
  providerBaseUrl,
  providerCanUseConfiguredKey,
  type ProviderModelGroups
} from "../providerProfiles";
import { ProviderConnectionFields } from "./ProviderConnectionFields";
import { SettingsEffortModelFields } from "./SettingsEffortModelFields";
import { ProviderModalityFields } from "./ProviderModalityFields";
import { ProviderModelInput as ModelSelect } from "./ProviderModelInput";

type SettingsModelsPanelProps = {
  canUseConfiguredKey: boolean;
  modelProfileCount: number;
  handleLoadProviderModels: () => Promise<void>;
  handleReloadProviderState: () => Promise<unknown>;
  handleSaveProviderConfig: () => Promise<void>;
  imageEndpointValidation: "idle" | "checking" | "valid" | "invalid";
  phase4: Phase4State | null;
  providerBusy: boolean;
  providerDraft: ProviderConfigInput | null;
  providerModelOptions: ProviderModelGroups;
  providerModels: string[];
  providerModelsBusy: boolean;
  providerModelsError: string | null;
  providerModelsRefreshTurn: number;
  providerSettingsError: string | null;
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function SettingsModelsPanel({
  canUseConfiguredKey,
  modelProfileCount,
  handleLoadProviderModels,
  handleReloadProviderState,
  handleSaveProviderConfig,
  imageEndpointValidation,
  phase4,
  providerBusy,
  providerDraft,
  providerModelOptions,
  providerModels,
  providerModelsBusy,
  providerModelsError,
  providerModelsRefreshTurn,
  providerSettingsError,
  setProviderDraft
}: SettingsModelsPanelProps) {
  const refreshAnimationClass =
    providerModelsRefreshTurn > 0 ? "settings-refresh-turn" : undefined;
  const resolvedBaseUrl = providerDraft
    ? providerBaseUrl(
        providerDraft.providerId,
        providerDraft.providerResource,
        providerDraft.baseUrl
      )
    : "";
  const providerBaseUrlReady = isValidProviderBaseUrl(resolvedBaseUrl);
  const credentialReady = Boolean(providerDraft?.apiKey.trim() || canUseConfiguredKey);
  const chatModelReady = Boolean(providerDraft?.executorModel.trim());
  const providerCanConnect = providerBaseUrlReady && credentialReady && chatModelReady;
  const configuredChatModels = providerDraft
    ? [
        providerDraft.model,
        providerDraft.conductorModel,
        providerDraft.plannerModel,
        providerDraft.executorModel,
        providerDraft.reviewerModel,
        providerDraft.summarizerModel
      ]
    : [];
  const multimodalModel =
    configuredChatModels.find((model) => providerModelOptions.multimodal.includes(model)) ??
    "Not active";
  const draftUsesSavedCredential = Boolean(
    providerDraft &&
      phase4?.provider &&
      providerCanUseConfiguredKey(providerDraft, phase4.provider) &&
      !providerDraft.apiKey.trim()
  );
  const credentialVerified = Boolean(
    draftUsesSavedCredential && phase4?.provider.authVerified
  );
  const legacyCredential = Boolean(
    draftUsesSavedCredential &&
      phase4?.provider.apiKeySet &&
      phase4.provider.authVerifiedAtMs === null
  );
  return (
    <>
      <section className="settings-section" data-settings-group="models">
        <div className="section-title">
          <KeyRound size={17} aria-hidden="true" />
          <h2>Provider</h2>
        </div>
        {providerDraft && (
          <div className="provider-form">
            <ProviderConnectionFields
              canUseConfiguredKey={canUseConfiguredKey}
              handleLoadProviderModels={handleLoadProviderModels}
              providerDraft={providerDraft}
              providerBusy={providerBusy}
              providerModels={providerModels}
              providerModelsBusy={providerModelsBusy}
              providerModelsError={providerModelsError}
              providerModelsRefreshTurn={providerModelsRefreshTurn}
              refreshAnimationClass={refreshAnimationClass}
              setProviderDraft={setProviderDraft}
            />
            <div>
              <label>
                <span>Context window</span>
                <select
                  disabled={providerBusy}
                  value={providerDraft.contextWindowTokens}
                  onChange={(event) =>
                    setProviderDraft({
                      ...providerDraft,
                      contextWindowTokens: Number(event.target.value)
                    })
                  }
                >
                  {[32768, 65536, 128000, 200000, 262144, 1000000, 1047576].map((tokens) => (
                    <option value={tokens} key={tokens}>
                      {tokens === 1047576
                        ? "1.05M"
                        : tokens >= 1000000
                          ? "1M"
                          : `${Math.round(tokens / 1000)}k`}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="provider-form provider-model-group">
              <div>
                <p className="settings-section-copy">
                  <strong>Model profiles</strong>
                  <br />
                  Configuration slots used by runtime stages, not independent agents.
                </p>
              </div>
              <div className="role-grid provider-meta-grid">
                <ModelSelect
                  label="Primary"
                  description="Default for general execution and Owner delivery."
                  value={providerDraft.executorModel}
                  options={providerModelOptions.chat}
                  disabled={providerBusy}
                  onChange={(executorModel) =>
                    setProviderDraft({ ...providerDraft, executorModel })
                  }
                />
                <ModelSelect
                  label="Reasoning"
                  description="Reasoning-heavy execution and Specialist analysis."
                  value={providerDraft.plannerModel}
                  options={providerModelOptions.chat}
                  disabled={providerBusy}
                  onChange={(plannerModel) => setProviderDraft({ ...providerDraft, plannerModel })}
                />
                <ModelSelect
                  label="Verifier"
                  description="Independent verification when a distinct verifier is used."
                  value={providerDraft.reviewerModel}
                  options={providerModelOptions.chat}
                  disabled={providerBusy}
                  onChange={(reviewerModel) =>
                    setProviderDraft({ ...providerDraft, reviewerModel })
                  }
                />
                <ModelSelect
                  label="Utility"
                  description="Summaries and other non-decision work."
                  value={providerDraft.summarizerModel}
                  options={providerModelOptions.chat}
                  disabled={providerBusy}
                  onChange={(summarizerModel) =>
                    setProviderDraft({ ...providerDraft, summarizerModel })
                  }
                />
              </div>
            </div>
            <div className="provider-form provider-model-group">
              <div>
                <p className="settings-section-copy">
                  <strong>Planning service override</strong>
                  <br />
                  Conductor plans work but does not own effects or final delivery.
                </p>
              </div>
              <ModelSelect
                label="Conductor"
                description="Dedicated model override for the planning service."
                value={providerDraft.conductorModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(conductorModel) =>
                  setProviderDraft({ ...providerDraft, conductorModel })
                }
              />
            </div>
            <SettingsEffortModelFields
              providerBusy={providerBusy}
              providerDraft={providerDraft}
              providerModelOptions={providerModelOptions}
              setProviderDraft={setProviderDraft}
            />
            <ProviderModalityFields
              imageEndpointValidation={imageEndpointValidation}
              providerBusy={providerBusy}
              providerDraft={providerDraft}
              providerModelOptions={providerModelOptions}
              providerModels={providerModels}
              setProviderDraft={setProviderDraft}
            />
            <dl className="settings-facts">
              <div>
                <dt>Connection</dt>
                <dd>
                  {credentialVerified
                    ? "Credential + Chat verified"
                    : legacyCredential
                      ? "Legacy key — reconnect to verify"
                      : "Not verified"}
                </dd>
              </div>
              <div>
                <dt>Multimodal</dt>
                <dd>{multimodalModel}</dd>
              </div>
              <div>
                <dt>Model allocation</dt>
                <dd>{modelProfileCount} unique models across 4 profiles</dd>
              </div>
            </dl>
            <button
              className="secondary-button"
              type="button"
              disabled={providerBusy || !providerCanConnect}
              onClick={handleSaveProviderConfig}
            >
              <Save size={17} aria-hidden="true" />
              <span>{providerBusy ? "Verifying" : "Connect provider"}</span>
            </button>
          </div>
        )}
        {providerSettingsError && (
          <div className="settings-inline-error" role="alert" aria-live="polite">
            {providerSettingsError}
          </div>
        )}
        {!providerDraft && providerSettingsError && (
          <button
            className="secondary-button"
            type="button"
            disabled={providerBusy}
            onClick={() => void handleReloadProviderState()}
          >
            <RefreshCw size={17} aria-hidden="true" />
            <span>Retry provider settings</span>
          </button>
        )}
      </section>
    </>
  );
}
