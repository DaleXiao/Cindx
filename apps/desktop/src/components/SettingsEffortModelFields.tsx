import type { Dispatch, SetStateAction } from "react";
import type { ProviderConfigInput } from "../tauri";
import {
  applyFastModelToProfiles,
  selectFastFallbackModel
} from "../providerModelAllocation";
import {
  providerModelContextWindow,
  providerTierDefaults,
  type ProviderModelGroups
} from "../providerProfiles";
import { ProviderModelInput as ModelSelect } from "./ProviderModelInput";

type SettingsEffortModelFieldsProps = {
  providerBusy: boolean;
  providerDraft: ProviderConfigInput;
  providerModelOptions: ProviderModelGroups;
  setProviderDraft: Dispatch<SetStateAction<ProviderConfigInput | null>>;
};

export function SettingsEffortModelFields({
  providerBusy,
  providerDraft,
  providerModelOptions,
  setProviderDraft
}: SettingsEffortModelFieldsProps) {
  const tierDefaults = providerTierDefaults(providerDraft.providerId);
  return (
    <>
      <div className="provider-form provider-model-group">
        <div>
          <p className="settings-section-copy">
            <strong>Effort tier defaults</strong>
            <br />
            Pinned defaults for the Fast, Auto, and Pro compute tiers. An
            empty pin uses the provider catalog tier default.
          </p>
        </div>
        <ModelSelect
          label="Fast tier"
          description={
            tierDefaults.fast
              ? `Empty uses ${tierDefaults.fast}.`
              : "Quick direct answer tier; empty falls back to the role slots."
          }
          value={providerDraft.fastModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          onChange={(fastModel) =>
            setProviderDraft({ ...providerDraft, fastModel })
          }
        />
        <ModelSelect
          label="Auto tier"
          description={
            tierDefaults.auto
              ? `Empty uses ${tierDefaults.auto}.`
              : "Adaptive verified tier; empty falls back to the role slots."
          }
          value={providerDraft.autoModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          onChange={(autoModel) =>
            setProviderDraft({ ...providerDraft, autoModel })
          }
        />
        <ModelSelect
          label="Pro tier"
          description={
            tierDefaults.pro
              ? `Empty uses ${tierDefaults.pro}.`
              : "Deep mission tier; empty falls back to the role slots."
          }
          value={providerDraft.proModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          onChange={(proModel) =>
            setProviderDraft({ ...providerDraft, proModel })
          }
        />
      </div>
      <div className="provider-form provider-model-group">
        <div>
          <p className="settings-section-copy">
            <strong>Compatibility fallback</strong>
            <br />
            Legacy primary slot used when a runtime stage has no dedicated
            model.
          </p>
        </div>
        <ModelSelect
          label={
            providerDraft.providerId === "azure_openai"
              ? "Fallback deployment"
              : "Fallback model"
          }
          description="Changing this fallback does not overwrite the profiles above."
          value={providerDraft.model}
          options={providerModelOptions.chat}
          disabled={providerBusy}
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
        <button
          className="secondary-button"
          type="button"
          disabled={providerBusy || !providerDraft.model.trim()}
          onClick={() =>
            setProviderDraft(applyFastModelToProfiles(providerDraft))
          }
        >
          Apply to all profiles
        </button>
      </div>
    </>
  );
}
