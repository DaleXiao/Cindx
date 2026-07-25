import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { AppearanceMode } from "../components/SettingsPage";
import {
  getPersonalizationConfig,
  savePersonalizationConfig,
  type PersonalizationConfig
} from "../tauri";

const APPEARANCE_STORAGE_KEY = "cindx.appearance";
const DEFAULT_PERSONALIZATION: PersonalizationConfig = {
  preferredName: "",
  responseTone: "natural",
  responseLength: "balanced"
};

function loadAppearanceMode(): AppearanceMode {
  if (typeof window === "undefined") return "system";
  try {
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    return stored === "light" || stored === "dark" || stored === "system"
      ? stored
      : "system";
  } catch {
    return "system";
  }
}

export function usePreferencesController() {
  const [personalizationDraft, setPersonalizationDraft] =
    useState<PersonalizationConfig>(DEFAULT_PERSONALIZATION);
  const [personalizationBusy, setPersonalizationBusy] = useState(false);
  const [personalizationError, setPersonalizationError] = useState<string | null>(null);
  const [appearanceMode, setAppearanceMode] = useState<AppearanceMode>(loadAppearanceMode);
  const [settingsToast, setSettingsToast] = useState<{
    id: number;
    message: string;
  } | null>(null);
  const settingsToastTimerRef = useRef<number | null>(null);
  const personalizationSaveTimerRef = useRef<number | null>(null);
  const personalizationSaveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const personalizationRevisionRef = useRef(0);
  const personalizationDraftRef = useRef<PersonalizationConfig>(DEFAULT_PERSONALIZATION);

  useLayoutEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyAppearance = () => {
      const resolved =
        appearanceMode === "system"
          ? systemTheme.matches
            ? "dark"
            : "light"
          : appearanceMode;
      document.documentElement.dataset.appearance = appearanceMode;
      document.documentElement.dataset.theme = resolved;
      document.documentElement.style.colorScheme = resolved;
    };
    applyAppearance();
    if (appearanceMode !== "system") return;
    systemTheme.addEventListener("change", applyAppearance);
    return () => systemTheme.removeEventListener("change", applyAppearance);
  }, [appearanceMode]);

  useEffect(
    () => () => {
      if (settingsToastTimerRef.current !== null) {
        window.clearTimeout(settingsToastTimerRef.current);
      }
      if (personalizationSaveTimerRef.current !== null) {
        window.clearTimeout(personalizationSaveTimerRef.current);
      }
    },
    []
  );

  const showSettingsSaved = useCallback((message = "Saved") => {
    if (settingsToastTimerRef.current !== null) {
      window.clearTimeout(settingsToastTimerRef.current);
    }
    setSettingsToast({ id: Date.now(), message });
    settingsToastTimerRef.current = window.setTimeout(() => {
      setSettingsToast(null);
      settingsToastTimerRef.current = null;
    }, 1800);
  }, []);

  const loadPersonalization = useCallback(async () => {
    const state = await getPersonalizationConfig();
    personalizationDraftRef.current = state;
    setPersonalizationDraft(state);
    return state;
  }, []);

  const handleAppearanceModeChange = useCallback(
    (mode: AppearanceMode) => {
      setAppearanceMode(mode);
      try {
        window.localStorage.setItem(APPEARANCE_STORAGE_KEY, mode);
      } catch {
        // Keep the preference for this app session when storage is unavailable.
      }
      showSettingsSaved("Appearance updated");
    },
    [showSettingsSaved]
  );

  const persistPersonalization = useCallback(
    (next: PersonalizationConfig, revision: number, notify: boolean) => {
      setPersonalizationBusy(true);
      setPersonalizationError(null);
      const save = personalizationSaveQueueRef.current
        .catch(() => undefined)
        .then(async () => {
          const saved = await savePersonalizationConfig(next);
          if (revision !== personalizationRevisionRef.current) return;
          personalizationDraftRef.current = saved;
          setPersonalizationDraft(saved);
          if (notify) showSettingsSaved("Personalization saved");
        })
        .catch((error) => {
          if (revision !== personalizationRevisionRef.current) return;
          setPersonalizationError(error instanceof Error ? error.message : String(error));
        })
        .finally(() => {
          if (revision === personalizationRevisionRef.current) {
            setPersonalizationBusy(false);
          }
        });
      personalizationSaveQueueRef.current = save;
      return save;
    },
    [showSettingsSaved]
  );

  const updatePersonalizationDraft = useCallback(
    (next: PersonalizationConfig) => {
      personalizationDraftRef.current = next;
      setPersonalizationDraft(next);
      setPersonalizationError(null);
      const revision = personalizationRevisionRef.current + 1;
      personalizationRevisionRef.current = revision;
      if (personalizationSaveTimerRef.current !== null) {
        window.clearTimeout(personalizationSaveTimerRef.current);
      }
      personalizationSaveTimerRef.current = window.setTimeout(() => {
        personalizationSaveTimerRef.current = null;
        void persistPersonalization(next, revision, false);
      }, 220);
    },
    [persistPersonalization]
  );

  const flushPersonalization = useCallback(
    (notify: boolean) => {
      if (personalizationSaveTimerRef.current !== null) {
        window.clearTimeout(personalizationSaveTimerRef.current);
        personalizationSaveTimerRef.current = null;
      }
      const revision = personalizationRevisionRef.current + 1;
      personalizationRevisionRef.current = revision;
      return persistPersonalization(personalizationDraftRef.current, revision, notify);
    },
    [persistPersonalization]
  );

  const handleSavePersonalization = useCallback(
    async () => flushPersonalization(true),
    [flushPersonalization]
  );

  return {
    appearanceMode,
    flushPersonalization,
    handleAppearanceModeChange,
    handleSavePersonalization,
    loadPersonalization,
    personalizationBusy,
    personalizationDraft,
    personalizationError,
    settingsToast,
    showSettingsSaved,
    updatePersonalizationDraft
  };
}
