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
  const chatStatus = (model: string, tier: string) => ({
    available: providerModels.includes(model),
    label: `${tier} model available through the latest authenticated provider connection`,
    title: `The selected ${tier} model is present in the latest authenticated model catalog`
  });
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
          description="Quick direct answer tier."
          value={providerDraft.fastModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          status={chatStatus(providerDraft.fastModel, "Fast tier")}
          onChange={(fastModel) =>
            setProviderDraft({ ...providerDraft, fastModel })
          }
        />
        <ModelSelect
          label="Auto tier"
          description="Adaptive verified tier."
          value={providerDraft.autoModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          status={chatStatus(providerDraft.autoModel, "Auto tier")}
          onChange={(autoModel) =>
            setProviderDraft({ ...providerDraft, autoModel })
          }
        />
        <ModelSelect
          label="Pro tier"
          description="Deep mission tier."
          value={providerDraft.proModel}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          status={chatStatus(providerDraft.proModel, "Pro tier")}
          onChange={(proModel) =>
            setProviderDraft({ ...providerDraft, proModel })
          }
        />
      </div>
      <div className="provider-form provider-model-group">
        <ModelSelect
          label={
            providerDraft.providerId === "azure_openai"
              ? "Fallback deployment"
              : "Fallback model"
          }
          value={providerDraft.model}
          options={providerModelOptions.chat}
          disabled={providerBusy}
          status={chatStatus(providerDraft.model, "Fallback")}
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
