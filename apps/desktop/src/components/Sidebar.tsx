import {
  Activity,
  FolderOpen,
  LayoutDashboard,
  MoreHorizontal,
  Plus,
  Search,
  Settings
} from "lucide-react";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { Menu } from "@tauri-apps/api/menu";
import type { ProjectView, SessionView } from "../tauri";

const appIconUrl = new URL("../../src-tauri/icons/icon.png", import.meta.url).href;

export type WorkspaceView = "timeline" | "trace" | "settings";

type SidebarProps = {
  activeView: WorkspaceView;
  projects: ProjectView[];
  sessions: SessionView[];
  busy: boolean;
  searchOpen: boolean;
  searchQuery: string;
  projectCreateOpen: boolean;
  projectName: string;
  onViewChange: (view: WorkspaceView) => void;
  onSearchToggle: () => void;
  onSearchQueryChange: (query: string) => void;
  onProjectCreateToggle: () => void;
  onProjectNameChange: (name: string) => void;
  onProjectCreate: () => void;
  onSessionCreate: () => void;
  onProjectSelect: (projectId: string) => void;
  onSessionSelect: (sessionId: string) => void;
  onSessionRename: (sessionId: string, name: string) => void;
  onSessionFork: (sessionId: string) => void;
  onSessionArchive: (sessionId: string) => void;
  onSessionDelete: (sessionId: string) => void;
};

export function Sidebar({
  activeView,
  projects,
  sessions,
  busy,
  searchOpen,
  searchQuery,
  projectCreateOpen,
  projectName,
  onViewChange,
  onSearchToggle,
  onSearchQueryChange,
  onProjectCreateToggle,
  onProjectNameChange,
  onProjectCreate,
  onSessionCreate,
  onProjectSelect,
  onSessionSelect,
  onSessionRename,
  onSessionFork,
  onSessionArchive,
  onSessionDelete
}: SidebarProps) {
  const activeProject = projects.find((project) => project.active) ?? null;

  async function openSessionMenu(session: SessionView, x: number, y: number) {
    const menu = await Menu.new({
      items: [
        {
          id: `rename-${session.id}`,
          text: "Rename",
          action: () => {
            const name = window.prompt("Rename session", session.name)?.trim();
            if (name && name !== session.name) {
              onSessionRename(session.id, name);
            }
          }
        },
        {
          id: `fork-${session.id}`,
          text: "Fork",
          action: () => onSessionFork(session.id)
        },
        {
          id: `archive-${session.id}`,
          text: "Archive",
          action: () => onSessionArchive(session.id)
        },
        {
          id: `delete-${session.id}`,
          text: "Delete",
          action: () => {
            if (window.confirm(`Delete “${session.name}”? This cannot be undone.`)) {
              onSessionDelete(session.id);
            }
          }
        }
      ]
    });
    try {
      await menu.popup(new LogicalPosition(x, y));
    } finally {
      await menu.close();
    }
  }

  return (
    <aside className="sidebar" aria-label="Projects and sessions">
      <div className="brand-row" title={activeProject?.root}>
        <img className="brand-mark" src={appIconUrl} alt="" />
        <div className="brand-name">Cindx</div>
        <button
          className={`icon-button brand-search ${searchOpen ? "active" : ""}`}
          aria-label="Search projects and sessions"
          title="Search"
          onClick={onSearchToggle}
          type="button"
        >
          <Search aria-hidden="true" />
        </button>
      </div>

      <div className="sidebar-actions" aria-label="Workspace views">
        <button
          className={`icon-button ${activeView === "timeline" ? "active" : ""}`}
          aria-label="Session thread"
          aria-pressed={activeView === "timeline"}
          title="Session"
          onClick={() => onViewChange("timeline")}
          type="button"
        >
          <LayoutDashboard aria-hidden="true" />
        </button>
        <button
          className={`icon-button ${activeView === "trace" ? "active" : ""}`}
          aria-label="Agent trace"
          aria-pressed={activeView === "trace"}
          title="Trace"
          onClick={() => onViewChange("trace")}
          type="button"
        >
          <Activity aria-hidden="true" />
        </button>
      </div>

      {searchOpen && (
        <label className="sidebar-search">
          <Search aria-hidden="true" />
          <input
            autoFocus
            value={searchQuery}
            onChange={(event) => onSearchQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") onSearchToggle();
            }}
            placeholder="Search"
            aria-label="Search projects and sessions"
          />
        </label>
      )}

      <section className="project-tree" aria-label="Project tree">
        <div className="nav-heading-row">
          <div className="nav-heading">Projects</div>
          <button
            className={`nav-add-button ${projectCreateOpen ? "active" : ""}`}
            type="button"
            aria-label="New project"
            title="New project"
            onClick={onProjectCreateToggle}
          >
            <Plus aria-hidden="true" />
          </button>
        </div>

        {projectCreateOpen && (
          <div className="quick-create">
            <input
              autoFocus
              value={projectName}
              onChange={(event) => onProjectNameChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") onProjectCreate();
                if (event.key === "Escape") onProjectCreateToggle();
              }}
              placeholder="Project name"
              aria-label="New project name"
            />
            <button
              type="button"
              className="icon-button"
              aria-label="Create project"
              disabled={busy || !projectName.trim()}
              onClick={onProjectCreate}
            >
              <FolderOpen aria-hidden="true" />
            </button>
          </div>
        )}

        <div className="project-list">
          {projects.length === 0 ? (
            <div className="nav-empty">No projects</div>
          ) : (
            projects.map((project) => (
              <div className="project-node" key={project.id}>
                <button
                  className={`nav-item project-item ${
                    project.active && activeView === "timeline" ? "active" : ""
                  }`}
                  onClick={() => onProjectSelect(project.id)}
                  type="button"
                  disabled={busy}
                  aria-expanded={project.active}
                  title={project.root}
                >
                  <span>
                    <strong>{project.name}</strong>
                  </span>
                  <em>{project.status}</em>
                </button>

                {project.active && (
                  <div className="session-branch">
                    <div className="nav-heading-row session-heading">
                      <div className="nav-heading">Sessions</div>
                      <button
                        className="nav-add-button"
                        type="button"
                        aria-label="New session"
                        title="New session"
                        disabled={busy}
                        onClick={onSessionCreate}
                      >
                        <Plus aria-hidden="true" />
                      </button>
                    </div>

                    <div className="session-list">
                      {sessions.length === 0 ? (
                        <div className="nav-empty">No sessions</div>
                      ) : (
                        sessions.map((session) => (
                          <div
                            className="session-row"
                            key={session.id}
                            onContextMenu={(event) => {
                              event.preventDefault();
                              void openSessionMenu(session, event.clientX, event.clientY);
                            }}
                          >
                            <button
                              className={`nav-item session-item ${
                                session.active && activeView === "timeline" ? "active" : ""
                              }`}
                              onClick={() => onSessionSelect(session.id)}
                              type="button"
                              disabled={busy}
                            >
                              <span>
                                <strong>{session.name}</strong>
                                <small>{session.detail}</small>
                              </span>
                              <em>{session.status}</em>
                            </button>
                            <button
                              className="session-more"
                              type="button"
                              aria-label={`Session actions for ${session.name}`}
                              title="Session actions"
                              disabled={busy}
                              onClick={(event) => {
                                event.stopPropagation();
                                const rect = event.currentTarget.getBoundingClientRect();
                                void openSessionMenu(session, rect.right, rect.bottom);
                              }}
                            >
                              <MoreHorizontal aria-hidden="true" />
                            </button>
                          </div>
                        ))
                      )}
                    </div>
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      </section>

      <div className="sidebar-footer">
        <button
          className={`icon-button sidebar-settings ${
            activeView === "settings" ? "active" : ""
          }`}
          aria-label="Settings"
          aria-pressed={activeView === "settings"}
          title="Settings"
          onClick={() => onViewChange("settings")}
          type="button"
        >
          <Settings aria-hidden="true" />
        </button>
      </div>
    </aside>
  );
}
