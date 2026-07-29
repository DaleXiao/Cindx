import { CheckCircle2, KeyRound, Save } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { Phase4State, ProviderConfigInput } from "../tauri";
import {
  isValidProviderBaseUrl,
  providerBaseUrl,
  providerCanUseConfiguredKey,
  providerModelContextWindow,
  providerSupportsWebRtcVoice,
  type ProviderModelGroups
} from "../providerProfiles";
import { PromptEvolutionPanel } from "./PromptEvolutionPanel";
import { ProviderConnectionFields } from "./ProviderConnectionFields";
import { ProviderModelInput as ModelSelect } from "./ProviderModelInput";

type SettingsModelsPanelProps = {
  canUseConfiguredKey: boolean;
  collaborationModelCount: number;
  handleLoadProviderModels: () => Promise<void>;
  handlePromptEvolutionToggle: (enabled: boolean) => Promise<void>;
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
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function SettingsModelsPanel({
  canUseConfiguredKey,
  collaborationModelCount,
  handleLoadProviderModels,
  handlePromptEvolutionToggle,
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
  setProviderDraft
}: SettingsModelsPanelProps) {
  const voiceSupported = providerDraft
    ? providerSupportsWebRtcVoice(providerDraft.providerId, providerDraft.baseUrl)
    : false;
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
            <div className="role-grid provider-meta-grid">
              <label>
                <span>Default effort</span>
                <select
                  disabled={providerBusy}
                  value={providerDraft.collaborationPolicy}
                  onChange={(event) =>
                    setProviderDraft({
                      ...providerDraft,
                      collaborationPolicy: event.target.value
                    })
                  }
                >
                  <option value="single">Cindx Fast</option>
                  <option value="auto_router">Cindx Auto</option>
                  <option value="best_of_n">Cindx Pro</option>
                </select>
              </label>
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
            <ModelSelect
              label={providerDraft.providerId === "azure_openai" ? "Default deployment" : "Default model"}
              value={providerDraft.model}
              options={providerModelOptions.chat}
              disabled={providerBusy}
              onChange={(model) =>
                setProviderDraft({
                  ...providerDraft,
                  model,
                  conductorModel: model,
                  plannerModel: model,
                  executorModel: model,
                  reviewerModel: model,
                  summarizerModel: model,
                  contextWindowTokens:
                    providerModelContextWindow(providerDraft.providerId, model) ??
                    providerDraft.contextWindowTokens
                })
              }
            />
            <div className="role-grid provider-meta-grid">
              <ModelSelect
                label="Conductor"
                value={providerDraft.conductorModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(conductorModel) =>
                  setProviderDraft({ ...providerDraft, conductorModel })
                }
              />
              <ModelSelect
                label="Planner"
                value={providerDraft.plannerModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(plannerModel) => setProviderDraft({ ...providerDraft, plannerModel })}
              />
              <ModelSelect
                label="Executor"
                value={providerDraft.executorModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(executorModel) => setProviderDraft({ ...providerDraft, executorModel })}
              />
              <ModelSelect
                label="Reviewer"
                value={providerDraft.reviewerModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(reviewerModel) => setProviderDraft({ ...providerDraft, reviewerModel })}
              />
              <ModelSelect
                label="Summary"
                value={providerDraft.summarizerModel}
                options={providerModelOptions.chat}
                disabled={providerBusy}
                onChange={(summarizerModel) =>
                  setProviderDraft({ ...providerDraft, summarizerModel })
                }
              />
              <ModelSelect
                label="Embedding"
                value={providerDraft.embeddingModel}
                options={providerModelOptions.embedding}
                disabled={providerBusy}
                onChange={(embeddingModel) =>
                  setProviderDraft({ ...providerDraft, embeddingModel })
                }
              />
              <ModelSelect
                label="Full-duplex voice"
                value={providerDraft.voiceModel}
                options={providerModelOptions.voice}
                emptyLabel={voiceSupported ? "Not configured" : "Not supported by this adapter"}
                disabled={providerBusy || !voiceSupported}
                onChange={(voiceModel) => setProviderDraft({ ...providerDraft, voiceModel })}
              />
              <ModelSelect
                label="Image generation"
                value={providerDraft.imageModel}
                options={providerModelOptions.image}
                disabled={providerBusy}
                emptyLabel="Not configured"
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
                Full-duplex voice is disabled because this provider needs a different realtime
                transport adapter.
              </p>
            )}
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
                <dt>Team</dt>
                <dd>{collaborationModelCount} unique models across 4 worker roles</dd>
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
      </section>

      <PromptEvolutionPanel
        phase4={phase4}
        providerBusy={providerBusy}
        providerDraft={providerDraft}
        onToggle={handlePromptEvolutionToggle}
      />
    </>
  );
}
