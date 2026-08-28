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
            <SettingsEffortModelFields
              providerBusy={providerBusy}
              providerDraft={providerDraft}
              providerModelOptions={providerModelOptions}
              providerModels={providerModels}
              setProviderDraft={setProviderDraft}
            />
            <label className="checkbox-row">
              <input
                type="checkbox"
                disabled={providerBusy}
                checked={providerDraft.directJudgeFailClosed}
                onChange={(event) =>
                  setProviderDraft({
                    ...providerDraft,
                    directJudgeFailClosed: event.target.checked
                  })
                }
              />
              <span>Fail closed when the delivery judge rejects a file-changing answer</span>
            </label>
            <label className="checkbox-row">
              <input
                type="checkbox"
                disabled={providerBusy}
                checked={providerDraft.guardianAutoApproval}
                onChange={(event) =>
                  setProviderDraft({
                    ...providerDraft,
                    guardianAutoApproval: event.target.checked
                  })
                }
              />
              <span>
                Guardian auto-approval: a distinct reviewer may approve non-destructive prompts
                (denials, timeouts, and malformed answers still fall back to you)
              </span>
            </label>
            <label className="checkbox-row">
              <input
                type="checkbox"
                disabled={providerBusy}
                checked={providerDraft.planFirstEnabled}
                onChange={(event) =>
                  setProviderDraft({
                    ...providerDraft,
                    planFirstEnabled: event.target.checked
                  })
                }
              />
              <span>Plan first (high/xhigh)</span>
            </label>
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
            </dl>
            <button
              className="secondary-button"
              type="button"
              disabled={providerBusy || !providerCanConnect}
              onClick={handleSaveProviderConfig}
            >
              <Save size={17} aria-hidden="true" />
              <span>{providerBusy ? "Verifying" : "Save"}</span>
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
