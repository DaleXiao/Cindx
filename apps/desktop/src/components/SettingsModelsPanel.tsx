import { CheckCircle2, KeyRound, RefreshCw, Save } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { Phase4State, ProviderConfigInput } from "../tauri";
import { PromptEvolutionPanel } from "./PromptEvolutionPanel";

type SettingsModelsPanelProps = {
  collaborationModelCount: number;
  handleLoadProviderModels: () => Promise<void>;
  handlePromptEvolutionToggle: (enabled: boolean) => Promise<void>;
  handleSaveProviderConfig: () => Promise<void>;
  imageEndpointValidation: "idle" | "checking" | "valid" | "invalid";
  phase4: Phase4State | null;
  providerBusy: boolean;
  providerDraft: ProviderConfigInput | null;
  providerModelOptions: string[];
  providerModels: string[];
  providerModelsBusy: boolean;
  providerModelsError: string | null;
  providerModelsRefreshTurn: number;
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

function ModelSelect({
  label,
  value,
  options,
  emptyLabel,
  onChange
}: {
  label: string;
  value: string;
  options: string[];
  emptyLabel?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label>
      <span>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)}>
        {emptyLabel && <option value="">{emptyLabel}</option>}
        {options.map((model) => (
          <option value={model} key={model}>
            {model}
          </option>
        ))}
      </select>
    </label>
  );
}

export function SettingsModelsPanel({
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
  return (
    <>
      <section className="settings-section" data-settings-group="models">
        <div className="section-title">
          <KeyRound size={17} aria-hidden="true" />
          <h2>Provider</h2>
        </div>
        {providerDraft && (
          <div className="provider-form">
            <label>
              <span>Base URL</span>
              <input
                value={providerDraft.baseUrl}
                onChange={(event) =>
                  setProviderDraft({ ...providerDraft, baseUrl: event.target.value })
                }
              />
            </label>
            <label>
              <span>API key</span>
              <input
                type="password"
                value={providerDraft.apiKey}
                autoComplete="off"
                spellCheck={false}
                placeholder={phase4?.provider.apiKeySet ? "Configured key" : "Enter API key"}
                onChange={(event) =>
                  setProviderDraft({ ...providerDraft, apiKey: event.target.value })
                }
              />
            </label>
            <div className="model-catalog-row">
              <button
                className="secondary-button"
                type="button"
                disabled={
                  providerModelsBusy ||
                  !providerDraft.baseUrl.trim() ||
                  (!providerDraft.apiKey.trim() && !phase4?.provider.apiKeySet)
                }
                onClick={() => void handleLoadProviderModels()}
              >
                <RefreshCw
                  aria-hidden="true"
                  className={providerModelsRefreshTurn > 0 ? "settings-refresh-turn" : undefined}
                  key={providerModelsRefreshTurn}
                />
                <span>{providerModelsBusy ? "Loading models" : "Load models"}</span>
              </button>
              <span>
                {providerModels.length > 0
                  ? `${providerModels.length} available`
                  : "Uses the provider model catalog"}
              </span>
            </div>
            {providerModelsError && (
              <div className="settings-inline-error">{providerModelsError}</div>
            )}
            <div className="role-grid provider-meta-grid">
              <label>
                <span>Default effort</span>
                <select
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
                  value={providerDraft.contextWindowTokens}
                  onChange={(event) =>
                    setProviderDraft({
                      ...providerDraft,
                      contextWindowTokens: Number(event.target.value)
                    })
                  }
                >
                  {[32768, 65536, 128000, 200000, 262144, 1000000].map((tokens) => (
                    <option value={tokens} key={tokens}>
                      {tokens >= 1000000 ? "1M" : `${Math.round(tokens / 1000)}k`}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <ModelSelect
              label="Default model"
              value={providerDraft.model}
              options={providerModelOptions}
              onChange={(model) =>
                setProviderDraft({
                  ...providerDraft,
                  model,
                  conductorModel: model,
                  plannerModel: model,
                  executorModel: model,
                  reviewerModel: model,
                  summarizerModel: model,
                  embeddingModel: providerDraft.embeddingModel
                })
              }
            />
            <div className="role-grid provider-meta-grid">
              <ModelSelect
                label="Conductor"
                value={providerDraft.conductorModel}
                options={providerModelOptions}
                onChange={(conductorModel) =>
                  setProviderDraft({ ...providerDraft, conductorModel })
                }
              />
              <ModelSelect
                label="Planner"
                value={providerDraft.plannerModel}
                options={providerModelOptions}
                onChange={(plannerModel) => setProviderDraft({ ...providerDraft, plannerModel })}
              />
              <ModelSelect
                label="Executor"
                value={providerDraft.executorModel}
                options={providerModelOptions}
                onChange={(executorModel) => setProviderDraft({ ...providerDraft, executorModel })}
              />
              <ModelSelect
                label="Reviewer"
                value={providerDraft.reviewerModel}
                options={providerModelOptions}
                onChange={(reviewerModel) => setProviderDraft({ ...providerDraft, reviewerModel })}
              />
              <ModelSelect
                label="Summary"
                value={providerDraft.summarizerModel}
                options={providerModelOptions}
                onChange={(summarizerModel) =>
                  setProviderDraft({ ...providerDraft, summarizerModel })
                }
              />
              <ModelSelect
                label="Embedding"
                value={providerDraft.embeddingModel}
                options={providerModelOptions}
                onChange={(embeddingModel) =>
                  setProviderDraft({ ...providerDraft, embeddingModel })
                }
              />
              <ModelSelect
                label="Full-duplex voice"
                value={providerDraft.voiceModel}
                options={providerModelOptions}
                emptyLabel="Not configured"
                onChange={(voiceModel) => setProviderDraft({ ...providerDraft, voiceModel })}
              />
              <ModelSelect
                label="Image generation"
                value={providerDraft.imageModel}
                options={providerModelOptions}
                emptyLabel="Not configured"
                onChange={(imageModel) => setProviderDraft({ ...providerDraft, imageModel })}
              />
              <label>
                <span>Image API endpoint</span>
                <div className="provider-endpoint-input" data-validation={imageEndpointValidation}>
                  <input
                    value={providerDraft.imageEndpoint}
                    spellCheck={false}
                    placeholder="Uses provider Base URL when empty"
                    onChange={(event) =>
                      setProviderDraft({
                        ...providerDraft,
                        imageEndpoint: event.target.value
                      })
                    }
                  />
                  {imageEndpointValidation === "valid" && (
                    <CheckCircle2
                      className="provider-endpoint-check"
                      aria-label="Image endpoint verified"
                    />
                  )}
                </div>
              </label>
            </div>
            <dl className="settings-facts">
              <div>
                <dt>Team</dt>
                <dd>{collaborationModelCount} unique models across 4 worker roles</dd>
              </div>
            </dl>
            <button
              className="secondary-button"
              type="button"
              disabled={providerBusy}
              onClick={handleSaveProviderConfig}
            >
              <Save size={17} aria-hidden="true" />
              <span>{providerBusy ? "Saving" : "Save provider"}</span>
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
