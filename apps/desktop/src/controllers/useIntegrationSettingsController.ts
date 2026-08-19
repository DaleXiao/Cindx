import { useCallback, useRef, useState } from "react";
import {
  getMcpState,
  getSidecarState,
  getSkillState,
  getWebSearchConfig,
  importExternalMcpServers,
  installSkillPackage,
  installSkillUrl,
  refreshMcpServer,
  refreshSkills,
  removeMcpServer,
  saveSidecarConfig,
  saveSkillPreference,
  saveWebSearchConfig,
  updateMcpServerPolicy,
  upsertMcpServer,
  type McpServerConfig,
  type McpState,
  type SidecarState,
  type SkillState,
  type WebSearchConfigState
} from "../tauri";
import { fileDataBase64 } from "../utils/fileDataBase64";

type IntegrationSettingsControllerOptions = {
  reportError: (message: string | null) => void;
  showSaved: (message?: string) => void;
};

export function useIntegrationSettingsController({
  reportError,
  showSaved
}: IntegrationSettingsControllerOptions) {
  const [sidecarState, setSidecarState] = useState<SidecarState | null>(null);
  const [sidecarDraft, setSidecarDraft] = useState({
    browserPath: "",
    computerPath: "",
    autoConfigure: true
  });
  const [sidecarBusy, setSidecarBusy] = useState(false);
  const [webSearchConfig, setWebSearchConfig] = useState<WebSearchConfigState | null>(null);
  const [webSearchDraft, setWebSearchDraft] = useState({ endpoint: "", apiKey: "" });
  const [webSearchBusy, setWebSearchBusy] = useState(false);
  const [webSearchError, setWebSearchError] = useState<string | null>(null);
  const [mcpState, setMcpState] = useState<McpState | null>(null);
  const [mcpBusy, setMcpBusy] = useState(false);
  const [mcpDraft, setMcpDraft] = useState({
    name: "",
    command: "",
    args: "",
    envKey: "",
    envValue: ""
  });
  const [skillState, setSkillState] = useState<SkillState | null>(null);
  const [skillBusy, setSkillBusy] = useState(false);
  const [skillRefreshTurn, setSkillRefreshTurn] = useState(0);
  const [skillUrl, setSkillUrl] = useState("");
  const [skillInstallError, setSkillInstallError] = useState<string | null>(null);
  const skillPackageInputRef = useRef<HTMLInputElement>(null);

  const loadIntegrationState = useCallback(() => {
    void getSidecarState()
      .then((state) => {
        setSidecarState(state);
        setSidecarDraft({
          browserPath: state.browser.path,
          computerPath: state.computer.path,
          autoConfigure: state.autoConfigure
        });
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
    void getWebSearchConfig()
      .then((state) => {
        setWebSearchConfig(state);
        setWebSearchDraft({ endpoint: state.endpoint, apiKey: "" });
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
    void getMcpState()
      .then((state) => {
        setMcpState(state);
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
    void getSkillState()
      .then((state) => {
        setSkillState(state);
        reportError(state.lastError);
      })
      .catch((error) => reportError(error instanceof Error ? error.message : String(error)));
  }, [reportError]);

  const handleSaveSidecars = useCallback(async () => {
    setSidecarBusy(true);
    reportError(null);
    try {
      const next = await saveSidecarConfig(sidecarDraft);
      setSidecarState(next);
      setSidecarDraft({
        browserPath: next.browser.path,
        computerPath: next.computer.path,
        autoConfigure: next.autoConfigure
      });
      reportError(next.lastError);
      showSaved();
    } finally {
      setSidecarBusy(false);
    }
  }, [reportError, showSaved, sidecarDraft]);

  const handleSaveWebSearch = useCallback(async () => {
    setWebSearchBusy(true);
    setWebSearchError(null);
    reportError(null);
    try {
      const next = await saveWebSearchConfig(webSearchDraft);
      setWebSearchConfig(next);
      setWebSearchDraft({ endpoint: next.endpoint, apiKey: "" });
      showSaved();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setWebSearchError(message);
      reportError(message);
    } finally {
      setWebSearchBusy(false);
    }
  }, [reportError, showSaved, webSearchDraft]);

  const handleAddMcpServer = useCallback(async () => {
    const name = mcpDraft.name.trim();
    const command = mcpDraft.command.trim();
    if (!name || !command) return;
    const slug =
      name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "mcp";
    const env =
      mcpDraft.envKey.trim() && mcpDraft.envValue
        ? { [mcpDraft.envKey.trim()]: mcpDraft.envValue }
        : {};
    const server: McpServerConfig = {
      id: `${slug}-${Date.now()}`,
      name,
      enabled: true,
      requireApproval: true,
      timeoutMs: 30000,
      transport: {
        type: "stdio",
        command,
        args: mcpDraft.args.split(/\s+/).filter(Boolean),
        env
      }
    };
    setMcpBusy(true);
    reportError(null);
    try {
      setMcpState(await upsertMcpServer(server));
      setMcpDraft({ name: "", command: "", args: "", envKey: "", envValue: "" });
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }, [mcpDraft, reportError]);

  const handleRefreshMcpServer = useCallback(
    async (serverId: string) => {
      setMcpBusy(true);
      reportError(null);
      try {
        const next = await refreshMcpServer(serverId);
        setMcpState(next);
        reportError(next.lastError);
      } catch (error) {
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setMcpBusy(false);
      }
    },
    [reportError]
  );

  const handleImportExternalMcpServers = useCallback(async () => {
    setMcpBusy(true);
    reportError(null);
    try {
      const next = await importExternalMcpServers();
      setMcpState(next);
      reportError(next.lastError);
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setMcpBusy(false);
    }
  }, [reportError]);

  const handleMcpPolicy = useCallback(
    async (serverId: string, enabled: boolean, requireApproval: boolean) => {
      setMcpBusy(true);
      try {
        setMcpState(await updateMcpServerPolicy(serverId, enabled, requireApproval));
      } catch (error) {
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setMcpBusy(false);
      }
    },
    [reportError]
  );

  const handleRemoveMcpServer = useCallback(
    async (serverId: string) => {
      setMcpBusy(true);
      try {
        setMcpState(await removeMcpServer(serverId));
      } catch (error) {
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setMcpBusy(false);
      }
    },
    [reportError]
  );

  const handleRefreshSkills = useCallback(async () => {
    setSkillRefreshTurn((current) => current + 1);
    setSkillBusy(true);
    try {
      setSkillState(await refreshSkills());
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setSkillBusy(false);
    }
  }, [reportError]);

  const handleSkillPreference = useCallback(
    async (skillId: string, enabled: boolean, trusted: boolean) => {
      setSkillBusy(true);
      try {
        setSkillState(await saveSkillPreference(skillId, enabled, trusted));
      } catch (error) {
        reportError(error instanceof Error ? error.message : String(error));
      } finally {
        setSkillBusy(false);
      }
    },
    [reportError]
  );

  const runSkillInstall = useCallback(
    async (request: () => Promise<SkillState>) => {
      setSkillBusy(true);
      setSkillInstallError(null);
      try {
        setSkillState(await request());
        showSaved("Skill installed");
        return true;
      } catch (error) {
        setSkillInstallError(error instanceof Error ? error.message : String(error));
        return false;
      } finally {
        setSkillBusy(false);
      }
    },
    [showSaved]
  );

  const handleInstallSkillPackage = useCallback(
    async (file: File) => {
      if (!/\.skill$/i.test(file.name)) {
        setSkillInstallError("Choose a .skill package.");
        return;
      }
      await runSkillInstall(async () => installSkillPackage(await fileDataBase64(file)));
    },
    [runSkillInstall]
  );

  const handleInstallSkillUrl = useCallback(async () => {
    const url = skillUrl.trim();
    if (!url) return;
    if (await runSkillInstall(() => installSkillUrl(url))) setSkillUrl("");
  }, [runSkillInstall, skillUrl]);

  return {
    handleAddMcpServer,
    handleImportExternalMcpServers,
    handleInstallSkillPackage,
    handleInstallSkillUrl,
    handleMcpPolicy,
    handleRefreshMcpServer,
    handleRefreshSkills,
    handleRemoveMcpServer,
    handleSaveSidecars,
    handleSaveWebSearch,
    handleSkillPreference,
    loadIntegrationState,
    mcpBusy,
    mcpDraft,
    mcpState,
    setMcpDraft,
    setSidecarDraft,
    setSkillUrl,
    setWebSearchDraft,
    sidecarBusy,
    sidecarDraft,
    sidecarState,
    skillBusy,
    skillInstallError,
    skillPackageInputRef,
    skillRefreshTurn,
    skillState,
    skillUrl,
    webSearchBusy,
    webSearchConfig,
    webSearchDraft,
    webSearchError
  };
}
