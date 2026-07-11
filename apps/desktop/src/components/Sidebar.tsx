import {
  Activity,
  Check,
  FolderOpen,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  Search,
  Settings,
  X
} from "lucide-react";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { Menu } from "@tauri-apps/api/menu";
import { useState } from "react";
import type { ProjectView, SessionView } from "../tauri";
import { DisclosureTriangle } from "./DisclosureTriangle";

const appIconUrl = new URL("../../src-tauri/icons/icon.png", import.meta.url).href;

type SessionVisualState = "working" | "complete" | "attention" | null;

function sessionVisualState(status: string): SessionVisualState {
  const normalized = status.trim().toLowerCase();
  if (["working", "running"].includes(normalized)) return "working";
  if (["completed", "succeeded", "done"].includes(normalized)) return "complete";
  if (
    [
      "review",
      "waiting_for_permission",
      "failed",
      "blocked",
      "cancelled",
      "attention"
    ].includes(normalized)
  ) {
    return "attention";
  }
  return null;
}

function SessionStatusIndicator({ status }: { status: string }) {
  const state = sessionVisualState(status);
  if (!state) return null;

  const label =
    state === "working"
      ? "Session is working"
      : state === "complete"
        ? "Session completed"
        : "Session needs attention";

  return (
    <span
      className={`session-status session-status-${state}`}
      role="status"
      aria-label={label}
      title={label}
    >
      {state === "working" ? (
        <LoaderCircle aria-hidden="true" />
      ) : (
        <span aria-hidden="true" />
      )}
    </span>
  );
}

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
  onProjectRename: (projectId: string, name: string) => void;
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
  onProjectRename,
  onSessionSelect,
  onSessionRename,
  onSessionFork,
  onSessionArchive,
  onSessionDelete
}: SidebarProps) {
  const activeProject = projects.find((project) => project.active) ?? null;
  const [collapsedProjectIds, setCollapsedProjectIds] = useState<Set<string>>(
    () => new Set()
  );
  const [renamingProjectId, setRenamingProjectId] = useState<string | null>(null);
  const [projectRenameDraft, setProjectRenameDraft] = useState("");
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const searchActive = searchOpen && Boolean(searchQuery.trim());

  function finishSearchSelection() {
    if (!searchOpen) return;
    onSearchQueryChange("");
    onSearchToggle();
  }

  function selectProjectResult(projectId: string) {
    onProjectSelect(projectId);
    finishSearchSelection();
  }

  function selectSessionResult(sessionId: string) {
    onSessionSelect(sessionId);
    finishSearchSelection();
  }

  function cancelSessionRename() {
    setRenamingSessionId(null);
    setRenameDraft("");
  }

  function cancelProjectRename() {
    setRenamingProjectId(null);
    setProjectRenameDraft("");
  }

  function commitProjectRename(project: ProjectView) {
    const name = projectRenameDraft.trim();
    if (!name) return;
    cancelProjectRename();
    if (name !== project.name) {
      onProjectRename(project.id, name);
    }
  }

  function commitSessionRename(session: SessionView) {
    const name = renameDraft.trim();
    if (!name) return;
    cancelSessionRename();
    if (name !== session.name) {
      onSessionRename(session.id, name);
    }
  }

  function handleProjectDisclosure(project: ProjectView, expanded: boolean) {
    setCollapsedProjectIds((current) => {
      const next = new Set(current);
      if (expanded) next.add(project.id);
      else next.delete(project.id);
      return next;
    });

    if (expanded || !project.active) cancelSessionRename();
    if (!project.active) onProjectSelect(project.id);
  }

  async function openSessionMenu(session: SessionView, x: number, y: number) {
    const menu = await Menu.new({
      items: [
        {
          id: `rename-${session.id}`,
          text: "Rename",
          action: () => {
            setRenameDraft(session.name);
            setRenamingSessionId(session.id);
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

  async function openProjectMenu(project: ProjectView, x: number, y: number) {
    const menu = await Menu.new({
      items: [
        {
          id: `rename-project-${project.id}`,
          text: "Rename",
          action: () => {
            cancelSessionRename();
            setProjectRenameDraft(project.name);
            setRenamingProjectId(project.id);
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

      {searchOpen && (
        <label className="sidebar-search">
          <Search aria-hidden="true" />
          <input
            autoFocus
            value={searchQuery}
            onChange={(event) => onSearchQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") onSearchToggle();
              if (event.key === "Enter" && searchActive) {
                event.preventDefault();
                const firstSession = sessions[0];
                if (firstSession) selectSessionResult(firstSession.id);
                else if (projects[0]) selectProjectResult(projects[0].id);
              }
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
            <div className="nav-empty">{searchActive ? "No results" : "No projects"}</div>
          ) : (
            projects.map((project) => {
              const projectSessions = sessions.filter(
                (session) => session.projectId === project.id
              );
              const expanded = searchActive
                ? projectSessions.length > 0
                : project.active && !collapsedProjectIds.has(project.id);

              return (
                <div className="project-node" key={project.id}>
                  <div
                    className={`project-row ${
                      project.active && activeView === "timeline" ? "active" : ""
                    }`}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      void openProjectMenu(project, event.clientX, event.clientY);
                    }}
                  >
                    <button
                      className="project-disclosure"
                      type="button"
                      disabled={busy}
                      aria-label={
                        searchActive
                          ? `Open ${project.name}`
                          : `${expanded ? "Collapse" : "Expand"} sessions for ${project.name}`
                      }
                      aria-expanded={expanded}
                      title={
                        searchActive
                          ? "Open project"
                          : expanded
                            ? "Collapse sessions"
                            : "Expand sessions"
                      }
                      onClick={() => {
                        if (searchActive) selectProjectResult(project.id);
                        else handleProjectDisclosure(project, expanded);
                      }}
                    >
                      <DisclosureTriangle />
                    </button>
                    {renamingProjectId === project.id ? (
                      <form
                        className="project-rename-form"
                        onSubmit={(event) => {
                          event.preventDefault();
                          commitProjectRename(project);
                        }}
                        onBlur={(event) => {
                          if (
                            !event.relatedTarget ||
                            !event.currentTarget.contains(event.relatedTarget as Node)
                          ) {
                            cancelProjectRename();
                          }
                        }}
                      >
                        <input
                          autoFocus
                          required
                          value={projectRenameDraft}
                          aria-label={`Rename ${project.name}`}
                          onChange={(event) => setProjectRenameDraft(event.target.value)}
                          onFocus={(event) => event.currentTarget.select()}
                          onKeyDown={(event) => {
                            if (event.key === "Escape") {
                              event.preventDefault();
                              cancelProjectRename();
                            }
                          }}
                        />
                        <button
                          type="submit"
                          aria-label="Save project name"
                          title="Save"
                          disabled={busy || !projectRenameDraft.trim()}
                          onPointerDown={(event) => event.preventDefault()}
                        >
                          <Check aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          aria-label="Cancel project rename"
                          title="Cancel"
                          onPointerDown={(event) => event.preventDefault()}
                          onClick={cancelProjectRename}
                        >
                          <X aria-hidden="true" />
                        </button>
                      </form>
                    ) : (
                      <>
                        <button
                          className="nav-item project-item"
                          onClick={() => selectProjectResult(project.id)}
                          type="button"
                          disabled={busy}
                          title={project.root}
                        >
                          <span>
                            <strong>{project.name}</strong>
                          </span>
                        </button>
                        <button
                          className="project-more"
                          type="button"
                          aria-label={`Project actions for ${project.name}`}
                          title="Project actions"
                          disabled={busy}
                          onClick={(event) => {
                            event.stopPropagation();
                            const rect = event.currentTarget.getBoundingClientRect();
                            void openProjectMenu(project, rect.right, rect.bottom);
                          }}
                        >
                          <MoreHorizontal aria-hidden="true" />
                        </button>
                      </>
                    )}
                  </div>

                  {expanded && (
                    <div className="session-branch">
                      {!searchActive && (
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
                      )}

                      <div className="session-list">
                        {projectSessions.length === 0 ? (
                          <div className="nav-empty">No sessions</div>
                        ) : (
                          projectSessions.map((session) => (
                          <div
                            className="session-row"
                            key={session.id}
                            onContextMenu={(event) => {
                              event.preventDefault();
                              void openSessionMenu(session, event.clientX, event.clientY);
                            }}
                          >
                            {renamingSessionId === session.id ? (
                              <form
                                className="session-rename-form"
                                onSubmit={(event) => {
                                  event.preventDefault();
                                  commitSessionRename(session);
                                }}
                                onBlur={(event) => {
                                  if (
                                    !event.relatedTarget ||
                                    !event.currentTarget.contains(event.relatedTarget as Node)
                                  ) {
                                    cancelSessionRename();
                                  }
                                }}
                              >
                                <input
                                  className="session-rename-input"
                                  autoFocus
                                  required
                                  value={renameDraft}
                                  aria-label={`Rename ${session.name}`}
                                  onChange={(event) => setRenameDraft(event.target.value)}
                                  onFocus={(event) => event.currentTarget.select()}
                                  onKeyDown={(event) => {
                                    if (event.key === "Escape") {
                                      event.preventDefault();
                                      cancelSessionRename();
                                    }
                                  }}
                                />
                                <button
                                  className="session-rename-action"
                                  type="submit"
                                  aria-label="Save session name"
                                  title="Save"
                                  disabled={busy || !renameDraft.trim()}
                                  onPointerDown={(event) => event.preventDefault()}
                                >
                                  <Check aria-hidden="true" />
                                </button>
                                <button
                                  className="session-rename-action"
                                  type="button"
                                  aria-label="Cancel session rename"
                                  title="Cancel"
                                  onPointerDown={(event) => event.preventDefault()}
                                  onClick={cancelSessionRename}
                                >
                                  <X aria-hidden="true" />
                                </button>
                              </form>
                            ) : (
                              <>
                                <button
                                  className={`nav-item session-item ${
                                    session.active && activeView === "timeline" ? "active" : ""
                                  }`}
                                  onClick={() => selectSessionResult(session.id)}
                                  type="button"
                                  disabled={busy}
                                >
                                  <span>
                                    <strong>{session.name}</strong>
                                  </span>
                                </button>
                                <SessionStatusIndicator status={session.status} />
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
                              </>
                            )}
                          </div>
                          ))
                        )}
                      </div>
                    </div>
                  )}
                </div>
              );
            })
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
        <button
          className={`icon-button sidebar-trace ${activeView === "trace" ? "active" : ""}`}
          aria-label="Agent trace"
          aria-pressed={activeView === "trace"}
          title="Agent trace"
          onClick={() => onViewChange("trace")}
          type="button"
        >
          <Activity aria-hidden="true" />
        </button>
      </div>
    </aside>
  );
}
