import { useEffect, useId, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { Check, ChevronDown, X } from "lucide-react";
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
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [open]);

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

  const remove = (model: string) => {
    const next = new Set(enabled);
    next.delete(model);
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
            Pick the models you want to use. Enabled models appear in the
            composer model picker; with nothing enabled, every catalog model is
            offered.
          </p>
        </div>
        <div className="settings-model-picker" ref={rootRef}>
          <button
            type="button"
            className="settings-model-picker-trigger"
            aria-haspopup="listbox"
            aria-expanded={open}
            aria-controls={listId}
            disabled={providerBusy}
            onClick={() => setOpen((current) => !current)}
          >
            <span>
              {enabled.size === 0
                ? "All catalog models"
                : `${enabled.size} model${enabled.size === 1 ? "" : "s"} enabled`}
            </span>
            <ChevronDown aria-hidden="true" />
          </button>
          {open && (
            <div
              className="settings-model-picker-menu"
              role="listbox"
              aria-multiselectable="true"
              aria-label="Enabled models"
              id={listId}
            >
              {catalog.length === 0 ? (
                <p className="settings-field-copy">
                  Load the provider's model catalog to pick models.
                </p>
              ) : (
                catalog.map((model) => {
                  const available = providerModels.includes(model);
                  const selected = enabled.has(model);
                  return (
                    <button
                      type="button"
                      role="option"
                      aria-selected={selected}
                      data-selected={selected}
                      className="settings-model-picker-option"
                      key={model}
                      disabled={providerBusy}
                      title={
                        available
                          ? `${model} is available through the latest authenticated provider connection`
                          : `${model} is not in the latest authenticated model catalog`
                      }
                      onClick={() => toggle(model)}
                    >
                      <span className="settings-model-picker-check" aria-hidden="true">
                        {selected && <Check />}
                      </span>
                      <span className={available ? undefined : "settings-checkbox-unavailable"}>
                        {model}
                        {!available && <small> (not in catalog)</small>}
                      </span>
                    </button>
                  );
                })
              )}
            </div>
          )}
        </div>
        {enabled.size > 0 && (
          <ul className="settings-model-chips" aria-label="Enabled models">
            {[...enabled].sort().map((model) => (
              <li key={model} className="settings-model-chip">
                <Check className="settings-model-chip-check" aria-hidden="true" />
                <span className="settings-model-chip-name">{model}</span>
                <button
                  type="button"
                  className="settings-model-chip-remove"
                  aria-label={`Disable ${model}`}
                  disabled={providerBusy}
                  onClick={() => remove(model)}
                >
                  <X aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        )}
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
