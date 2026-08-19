import { ArchiveRestore, Trash2 } from "lucide-react";
import {
  confirmDeleteAction,
  deleteSession,
  type ProjectSessionState,
  type SessionView
} from "../tauri";

export type SettingsSessionsPanelProps = {
  archivedSessions: SessionView[];
  projectSessionState: ProjectSessionState | null;
  projectSessionBusy: boolean;
  handleRestoreSession: (sessionId: string) => Promise<void>;
  handleDeleteSession: (sessionId: string) => Promise<void>;
  completeBulkDelete: (
    ids: string[],
    next: ProjectSessionState | null,
    error: string | null
  ) => void;
};

function formatArchivedTime(timestampMs: number | null) {
  if (timestampMs == null) return "";
  return new Date(timestampMs).toLocaleString();
}

export function SettingsSessionsPanel({
  archivedSessions,
  projectSessionState,
  projectSessionBusy,
  handleRestoreSession,
  handleDeleteSession,
  completeBulkDelete
}: SettingsSessionsPanelProps) {
  // Delete each archived session with a single fast command apiece, then run
  // one workspace refresh at the end instead of one per deletion.
  const deleteAllArchived = async () => {
    const ids: string[] = [];
    let next: ProjectSessionState | null = null;
    let error: string | null = null;
    for (const session of archivedSessions) {
      try {
        next = await deleteSession(session.id);
        ids.push(session.id);
      } catch (err) {
        error = err instanceof Error ? err.message : String(err);
        break;
      }
    }
    completeBulkDelete(ids, next, error);
  };
  return (
    <section className="settings-section" data-settings-group="sessions">
      <div className="section-title">
        <ArchiveRestore aria-hidden="true" />
        <h2>Archived sessions</h2>
        {archivedSessions.length > 0 && (
          <button
            className="secondary-button archived-delete-all"
            type="button"
            disabled={projectSessionBusy}
            onClick={() => {
              void (async () => {
                const confirmed = await confirmDeleteAction(
                  "sessions",
                  `${archivedSessions.length} archived session${
                    archivedSessions.length === 1 ? "" : "s"
                  }`
                );
                if (confirmed) await deleteAllArchived();
              })();
            }}
          >
            <Trash2 aria-hidden="true" />
            <span>Delete all</span>
          </button>
        )}
      </div>
      {archivedSessions.length === 0 ? (
        <div className="settings-empty">No archived sessions</div>
      ) : (
        <div className="archived-session-list">
          {archivedSessions.map((session) => (
            <div className="archived-session-row" key={session.id}>
              <span>
                <strong>{session.name}</strong>
                <small>
                  {projectSessionState?.projects.find(
                    (project) => project.id === session.projectId
                  )?.name ?? "Project"}
                  {session.archivedAtMs
                    ? ` · ${formatArchivedTime(session.archivedAtMs)}`
                    : ""}
                </small>
              </span>
              <div className="archived-session-actions">
                <button
                  className="secondary-button"
                  type="button"
                  disabled={projectSessionBusy}
                  onClick={() => void handleRestoreSession(session.id)}
                >
                  <ArchiveRestore aria-hidden="true" />
                  <span>Restore</span>
                </button>
                <button
                  className="secondary-button archived-session-delete"
                  type="button"
                  disabled={projectSessionBusy}
                  onClick={() => {
                    void (async () => {
                      const confirmed = await confirmDeleteAction(
                        "session",
                        session.name
                      );
                      if (confirmed) await handleDeleteSession(session.id);
                    })();
                  }}
                >
                  <Trash2 aria-hidden="true" />
                  <span>Delete</span>
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
