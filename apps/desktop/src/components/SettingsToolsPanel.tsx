import {
  Camera,
  ChevronDown,
  ChevronRight,
  FileText,
  Globe2,
  Keyboard,
  LayoutDashboard,
  MousePointerClick,
  MoveVertical,
  PanelRightOpen,
  Save,
  Search,
  Wrench
} from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type {
  Phase5State,
  RuntimeStatus,
  SidecarState,
  ToolSpecView,
  WebSearchConfigState
} from "../tauri";

export type WebSearchDraft = {
  endpoint: string;
  apiKey: string;
};

type SettingsToolsPanelProps = {
  browserBusy: boolean;
  browserTarget: string;
  browserText: string;
  browserUrl: string;
  onRunBrowserTool: (toolName: string) => Promise<void>;
  onRunTool: () => Promise<void>;
  onSaveWebSearch: () => Promise<void>;
  phase5: Phase5State | null;
  runtime: RuntimeStatus | null;
  selectedTool: string;
  selectedToolSpec: ToolSpecView | undefined;
  setBrowserTarget: Dispatch<SetStateAction<string>>;
  setBrowserText: Dispatch<SetStateAction<string>>;
  setBrowserUrl: Dispatch<SetStateAction<string>>;
  setSelectedTool: Dispatch<SetStateAction<string>>;
  setToolInput: Dispatch<SetStateAction<string>>;
  setWebSearchDraft: Dispatch<SetStateAction<WebSearchDraft>>;
  sidecarState: SidecarState | null;
  toolBusy: boolean;
  toolInput: string;
  webSearchBusy: boolean;
  webSearchConfig: WebSearchConfigState | null;
  webSearchDraft: WebSearchDraft;
  webSearchError: string | null;
};

const DEFAULT_TOOL_INPUTS: Record<string, string> = {
  "file.read": "path=README.md",
  "file.list": "path=.",
  "file.search": "path=.\nquery=Phase",
  "file.write": "path=.cindx/demo.txt\ncontent=hello from Cindx",
  "shell.run": "command=pwd\ncwd=.",
  "web.search": "query=local agent",
  "browser.open": "url=https://example.com",
  "browser.extract_text": "url=https://example.com",
  "browser.capture": "url=https://example.com\noutput_dir=.cindx/browser-captures",
  "browser.click":
    "url=https://example.com\nselector=body\noutput_dir=.cindx/browser-actions",
  "browser.type":
    "url=https://example.com\nselector=body\ntext=hello\noutput_dir=.cindx/browser-actions",
  "browser.scroll":
    "url=https://example.com\ndelta_y=600\noutput_dir=.cindx/browser-actions",
  "browser.tabs": "",
  "browser.select_tab": "tab_id=<copy from browser.tabs>",
  "computer.screenshot": "redaction=manual\noutput_dir=.cindx/computer-actions",
  "computer.click": "x=120\ny=240\noutput_dir=.cindx/computer-actions",
  "computer.type": "text=hello\noutput_dir=.cindx/computer-actions",
  "computer.key": "key=Cmd+S\ndestructive=false\noutput_dir=.cindx/computer-actions",
  "computer.scroll": "delta_y=600\noutput_dir=.cindx/computer-actions"
};

function SettingsChevron({ action = false }: { action?: boolean }) {
  return (
    <span
      className={action ? "settings-action-chevron" : "settings-disclosure-chevron"}
      aria-hidden="true"
    >
      {action ? <ChevronRight /> : <ChevronDown />}
    </span>
  );
}

export function SettingsToolsPanel({
  browserBusy,
  browserTarget,
  browserText,
  browserUrl,
  onRunBrowserTool,
  onRunTool,
  onSaveWebSearch,
  phase5,
  runtime,
  selectedTool,
  selectedToolSpec,
  setBrowserTarget,
  setBrowserText,
  setBrowserUrl,
  setSelectedTool,
  setToolInput,
  setWebSearchDraft,
  sidecarState,
  toolBusy,
  toolInput,
  webSearchBusy,
  webSearchConfig,
  webSearchDraft,
  webSearchError
}: SettingsToolsPanelProps) {
  const registeredToolCount = phase5?.tools.length ?? runtime?.registeredTools.length ?? 0;

  return (
    <>
      <section className="settings-section" data-settings-group="tools">
        <div className="section-title">
          <Globe2 size={17} aria-hidden="true" />
          <h2>Web search API</h2>
        </div>
        <div className="provider-form">
          <label>
            <span>Endpoint</span>
            <input
              value={webSearchDraft.endpoint}
              placeholder="https://search.example.com/api"
              onChange={(event) =>
                setWebSearchDraft({ ...webSearchDraft, endpoint: event.target.value })
              }
            />
          </label>
          <label>
            <span>API key</span>
            <input
              type="password"
              value={webSearchDraft.apiKey}
              autoComplete="off"
              placeholder={webSearchConfig?.apiKeySet ? "Configured" : "Optional"}
              onChange={(event) =>
                setWebSearchDraft({ ...webSearchDraft, apiKey: event.target.value })
              }
            />
          </label>
          <button
            className="secondary-button"
            type="button"
            disabled={webSearchBusy}
            onClick={() => void onSaveWebSearch()}
          >
            <Save aria-hidden="true" />
            <span>{webSearchBusy ? "Saving" : "Save web search"}</span>
          </button>
        </div>
        {webSearchError && <div className="settings-inline-error">{webSearchError}</div>}
      </section>

      <section className="settings-section" data-settings-group="tools">
        <div className="section-title">
          <Globe2 size={17} aria-hidden="true" />
          <h2>Browser</h2>
        </div>
        <dl className="settings-facts">
          <div>
            <dt>Status</dt>
            <dd>{sidecarState?.browser.healthy ? "Ready" : "Check configuration"}</dd>
          </div>
          <div>
            <dt>Controller</dt>
            <dd>Local sidecar</dd>
          </div>
          <div>
            <dt>Permission</dt>
            <dd>Reviewed</dd>
          </div>
        </dl>
        <details className="advanced-settings registered-tools-details">
          <summary>
            <span>Registered tools</span>
            <SettingsChevron />
            <strong>{registeredToolCount}</strong>
          </summary>
          <div className="registered-tool-list">
            {(phase5?.tools ?? []).length > 0
              ? phase5?.tools.map((tool) => (
                  <div className="registered-tool-row" key={tool.name}>
                    <span>
                      <strong>{tool.name}</strong>
                      <small>{tool.description}</small>
                    </span>
                    <em>{tool.risk}</em>
                  </div>
                ))
              : runtime?.registeredTools.map((tool) => (
                  <div className="registered-tool-row" key={tool}>
                    <span>
                      <strong>{tool}</strong>
                    </span>
                  </div>
                ))}
          </div>
        </details>
        <details className="advanced-settings">
          <summary>
            <span>Manual browser controls</span>
            <SettingsChevron />
          </summary>
          <div className="tool-runner">
            <label>
              <span>URL or query</span>
              <input value={browserUrl} onChange={(event) => setBrowserUrl(event.target.value)} />
            </label>
            <label>
              <span>Target or tab ID</span>
              <input
                value={browserTarget}
                onChange={(event) => setBrowserTarget(event.target.value)}
              />
            </label>
            <label>
              <span>Text</span>
              <input value={browserText} onChange={(event) => setBrowserText(event.target.value)} />
            </label>
            <div className="button-row">
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("web.search")}
              >
                <Search size={17} aria-hidden="true" />
                <span>Search web</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.open")}
              >
                <Globe2 size={17} aria-hidden="true" />
                <span>Open</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.extract_text")}
              >
                <FileText size={17} aria-hidden="true" />
                <span>Extract</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.capture")}
              >
                <Camera size={17} aria-hidden="true" />
                <span>Capture</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.click")}
              >
                <MousePointerClick size={17} aria-hidden="true" />
                <span>Click</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.type")}
              >
                <Keyboard size={17} aria-hidden="true" />
                <span>Type</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserUrl.trim()}
                onClick={() => onRunBrowserTool("browser.scroll")}
              >
                <MoveVertical size={17} aria-hidden="true" />
                <span>Scroll</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy}
                onClick={() => onRunBrowserTool("browser.tabs")}
              >
                <LayoutDashboard size={17} aria-hidden="true" />
                <span>List tabs</span>
              </button>
              <button
                className="secondary-button"
                type="button"
                disabled={browserBusy || !browserTarget.trim()}
                onClick={() => onRunBrowserTool("browser.select_tab")}
              >
                <PanelRightOpen size={17} aria-hidden="true" />
                <span>Select tab</span>
              </button>
            </div>
          </div>
        </details>
      </section>

      <section className="settings-section" data-settings-group="tools">
        <div className="section-title">
          <Wrench size={17} aria-hidden="true" />
          <h2>Tools</h2>
        </div>
        <dl className="settings-facts">
          <div>
            <dt>Registered</dt>
            <dd>{registeredToolCount}</dd>
          </div>
          <div>
            <dt>Scope</dt>
            <dd>Active workspace</dd>
          </div>
          <div>
            <dt>Execution</dt>
            <dd>Permission gated</dd>
          </div>
        </dl>
        <details className="advanced-settings">
          <summary>
            <span>Manual tool runner</span>
            <SettingsChevron />
          </summary>
          <div className="tool-runner">
            <label>
              <span>Tool</span>
              <select
                value={selectedTool}
                onChange={(event) => {
                  const nextTool = event.target.value;
                  setSelectedTool(nextTool);
                  if (Object.prototype.hasOwnProperty.call(DEFAULT_TOOL_INPUTS, nextTool)) {
                    setToolInput(DEFAULT_TOOL_INPUTS[nextTool]);
                  }
                }}
              >
                {(phase5?.tools ?? []).map((tool) => (
                  <option key={tool.name} value={tool.name}>
                    {tool.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Input</span>
              <textarea
                value={toolInput}
                onChange={(event) => setToolInput(event.target.value)}
                rows={4}
              />
            </label>
            {selectedToolSpec && (
              <div className="tool-schema">
                <strong>{selectedToolSpec.risk}</strong>
                <span>{selectedToolSpec.inputSchema}</span>
              </div>
            )}
            <button
              className="secondary-button"
              type="button"
              disabled={toolBusy}
              onClick={onRunTool}
            >
              <span>{toolBusy ? "Running" : "Run tool"}</span>
              <SettingsChevron action />
            </button>
          </div>
        </details>
      </section>
    </>
  );
}
