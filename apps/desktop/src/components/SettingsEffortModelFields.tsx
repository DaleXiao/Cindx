import type { Dispatch, SetStateAction } from "react";
import type { ProviderConfigInput } from "../tauri";
import { selectFastFallbackModel } from "../providerModelAllocation";
import {
  providerModelContextWindow,
  type ProviderModelGroups
} from "../providerProfiles";
import { ProviderModelInput as ModelSelect } from "./ProviderModelInput";

type SettingsEffortModelFieldsProps = {
  providerBusy: boolean;
  providerDraft: ProviderConfigInput;
  providerModelOptions: ProviderModelGroups;
  providerModels: string[];
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function SettingsEffortModelFields({
  providerBusy,
  providerDraft,
  providerModelOptions,
  providerModels,
  setProviderDraft
}: SettingsEffortModelFieldsProps) {
  const enabled = new Set(providerDraft.enabledModels);
  const catalog = providerModelOptions.chat;

  const toggle = (model: string) => {
    const next = new Set(enabled);
    if (next.has(model)) {
      next.delete(model);
    } else {
      next.add(model);
    }
    setProviderDraft({
      ...providerDraft,
      enabledModels: [...next].sort()
    });
  };

  return (
    <>
      <div className="provider-form provider-model-group">
        <div>
          <p className="settings-section-copy">
            <strong>Enabled models</strong>
            <br />
            Check the models you want to use. Checked models appear in the
            composer model picker; with nothing checked, every catalog model is
            offered.
          </p>
        </div>
        <div className="settings-checkbox-list" role="group" aria-label="Enabled models">
          {catalog.length === 0 ? (
            <p className="settings-field-copy">
              Load the provider's model catalog to pick models.
            </p>
          ) : (
            catalog.map((model) => {
              const available = providerModels.includes(model);
              return (
                <label
                  key={model}
                  className="settings-checkbox-row"
                  title={
                    available
                      ? `${model} is available through the latest authenticated provider connection`
                      : `${model} is not in the latest authenticated model catalog`
                  }
                >
                  <input
                    type="checkbox"
                    checked={enabled.has(model)}
                    disabled={providerBusy}
                    onChange={() => toggle(model)}
                  />
                  <span className={available ? undefined : "settings-checkbox-unavailable"}>
                    {model}
                    {!available && <small> (not in catalog)</small>}
                  </span>
                </label>
              );
            })
          )}
        </div>
      </div>
      <div className="provider-form provider-model-group">
        <ModelSelect
          label={
            providerDraft.providerId === "azure_openai"
              ? "Fallback deployment"
              : "Fallback model"
          }
          description="Used when no model is explicitly selected for a run."
          value={providerDraft.model}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          status={{
            available: providerModels.includes(providerDraft.model),
            label: "Fallback model available through the latest authenticated provider connection",
            title:
              "The fallback model is present in the latest authenticated model catalog"
          }}
          onChange={(model) =>
            setProviderDraft(
              selectFastFallbackModel(
                providerDraft,
                model,
                providerModelContextWindow(providerDraft.providerId, model)
              )
            )
          }
        />
      </div>
    </>
  );
}
