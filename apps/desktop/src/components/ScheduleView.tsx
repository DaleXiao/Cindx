import {
  ArrowLeft,
  ArrowRight,
  CalendarClock,
  CheckCircle2,
  CircleAlert,
  Clock3,
  Pencil,
  Play,
  Plus,
  Save,
  Trash2,
  X,
  XCircle
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import {
  cancelScheduleRun,
  deleteSchedule,
  getScheduleState,
  runScheduleNow,
  setScheduleEnabled,
  upsertSchedule,
  type AgentEffort,
  type ProjectView,
  type ScheduleCadence,
  type ScheduleState,
  type ScheduleView as ScheduleRecord,
  type SessionView,
  type UpsertScheduleInput
} from "../tauri";

type ScheduleDraft = {
  id: string | null;
  name: string;
  projectId: string;
  sessionId: string;
  prompt: string;
  effort: AgentEffort;
  timezone: string;
  cadence: ScheduleCadence;
  startLocal: string;
  weeklyDays: number[];
  endsLocal: string;
  catchUp: boolean;
  enabled: boolean;
};

const cadenceOptions: Array<{ value: ScheduleCadence; label: string }> = [
  { value: "once", label: "Once" },
  { value: "daily", label: "Daily" },
  { value: "weekdays", label: "Weekdays" },
  { value: "weekly", label: "Weekly" }
];

const effortOptions: Array<{ value: AgentEffort; label: string }> = [
  { value: "fast", label: "Fast" },
  { value: "auto", label: "Auto" },
  { value: "pro", label: "Pro" }
];

const weekdayOptions = [
  { value: 1, label: "Mon" },
  { value: 2, label: "Tue" },
  { value: 3, label: "Wed" },
  { value: 4, label: "Thu" },
  { value: 5, label: "Fri" },
  { value: 6, label: "Sat" },
  { value: 7, label: "Sun" }
];

const activeRunStatuses = new Set([
  "preparing",
  "queued",
  "running",
  "waiting_for_permission"
]);

function localDateTimeValue(timestampMs: number) {
  const date = new Date(timestampMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}`;
}

function defaultStartValue() {
  const date = new Date(Date.now() + 5 * 60_000);
  date.setSeconds(0, 0);
  return localDateTimeValue(date.getTime());
}

function currentTimeZone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

function currentWeekday() {
  return ((new Date().getDay() + 6) % 7) + 1;
}

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "Not scheduled";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short"
  }).format(new Date(timestampMs));
}

function runStatusLabel(status: string) {
  if (status === "waiting_for_permission") return "Needs approval";
  return status.charAt(0).toUpperCase() + status.slice(1);
}

function draftForNew(): ScheduleDraft {
  return {
    id: null,
    name: "",
    projectId: "",
    sessionId: "",
    prompt: "",
    effort: "auto",
    timezone: currentTimeZone(),
    cadence: "once",
    startLocal: defaultStartValue(),
    weeklyDays: [currentWeekday()],
    endsLocal: "",
    catchUp: true,
    enabled: true
  };
}

function draftForSchedule(schedule: ScheduleRecord): ScheduleDraft {
  return {
    id: schedule.id,
    name: schedule.name,
    projectId: schedule.projectId ?? "",
    sessionId: schedule.sessionId ?? "",
    prompt: schedule.prompt,
    effort: schedule.effort,
    timezone: currentTimeZone(),
    cadence: schedule.cadence,
    startLocal: localDateTimeValue(schedule.anchorAtMs),
    weeklyDays: schedule.weeklyDays.length ? schedule.weeklyDays : [currentWeekday()],
    endsLocal: schedule.endsAtMs ? localDateTimeValue(schedule.endsAtMs) : "",
    catchUp: schedule.catchUp,
    enabled: schedule.enabled
  };
}

type ScheduleViewProps = {
  projects: ProjectView[];
  sessions: SessionView[];
  onOpenSession: (sessionId: string) => void;
  onBack: () => void;
  requestedScheduleId: string | null;
  onScheduleSelect: (scheduleId: string | null) => void;
};

export function ScheduleView({
  projects,
  sessions,
  onOpenSession,
  onBack,
  requestedScheduleId,
  onScheduleSelect
}: ScheduleViewProps) {
  const [state, setState] = useState<ScheduleState | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(requestedScheduleId);
  const [draft, setDraft] = useState<ScheduleDraft>(draftForNew);
  const [editing, setEditing] = useState(false);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ScheduleRecord | null>(null);

  useEffect(() => {
    let disposed = false;
    const refresh = async () => {
      try {
        const next = await getScheduleState();
        if (disposed) return;
        setState(next);
        setError((current) => current ?? next.lastError);
        setSelectedId((current) =>
          current && next.schedules.some((schedule) => schedule.id === current)
            ? current
            : next.schedules[0]?.id ?? null
        );
      } catch (loadError) {
        if (!disposed) {
          setError(loadError instanceof Error ? loadError.message : String(loadError));
        }
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (!requestedScheduleId) return;
    if (state?.schedules.some((schedule) => schedule.id === requestedScheduleId)) {
      setSelectedId(requestedScheduleId);
      setEditing(false);
    }
  }, [requestedScheduleId, state]);

  const selected =
    state?.schedules.find((schedule) => schedule.id === selectedId) ?? null;
  const projectSessions = useMemo(
    () =>
      sessions.filter(
        (session) => session.projectId === draft.projectId && !session.archived
      ),
    [draft.projectId, sessions]
  );

  function beginNew() {
    setDraft(draftForNew());
    setEditing(true);
    setError(null);
  }

  function beginEdit(schedule: ScheduleRecord) {
    setSelectedId(schedule.id);
    onScheduleSelect(schedule.id);
    setDraft(draftForSchedule(schedule));
    setEditing(true);
    setError(null);
  }

  async function perform(action: string, operation: () => Promise<ScheduleState>) {
    setBusyAction(action);
    setError(null);
    try {
      const next = await operation();
      setState(next);
      return next;
    } catch (actionError) {
      setError(actionError instanceof Error ? actionError.message : String(actionError));
      return null;
    } finally {
      setBusyAction(null);
    }
  }

  async function saveDraft() {
    if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(draft.startLocal)) {
      setError("Choose a valid start time.");
      return;
    }
    if (
      draft.endsLocal &&
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(draft.endsLocal)
    ) {
      setError("Choose a valid end time.");
      return;
    }
    if (draft.cadence === "weekly" && draft.weeklyDays.length === 0) {
      setError("Choose at least one day for a weekly schedule.");
      return;
    }
    const input: UpsertScheduleInput = {
      id: draft.id,
      name: draft.name,
      projectId: draft.projectId || null,
      sessionId: draft.sessionId || null,
      prompt: draft.prompt,
      effort: draft.effort,
      timezone: draft.timezone,
      cadence: draft.cadence,
      anchorLocal: draft.startLocal,
      weeklyDays: draft.weeklyDays,
      endsLocal: draft.endsLocal || null,
      catchUp: draft.catchUp,
      enabled: draft.enabled
    };
    const next = await perform("save", () => upsertSchedule(input));
    if (!next) return;
    const saved = draft.id
      ? next.schedules.find((schedule) => schedule.id === draft.id)
      : [...next.schedules]
          .filter(
            (schedule) =>
              schedule.name === draft.name.trim() &&
              schedule.sessionId === (draft.sessionId || null)
          )
          .sort((left, right) => right.updatedAtMs - left.updatedAtMs)[0];
    setSelectedId(saved?.id ?? next.schedules[0]?.id ?? null);
    onScheduleSelect(saved?.id ?? next.schedules[0]?.id ?? null);
    setEditing(false);
  }

  const latestRun = selected?.runs[selected.runs.length - 1] ?? null;
  const activeRun = latestRun && activeRunStatuses.has(latestRun.status) ? latestRun : null;

  return (
    <section className="schedule-view" aria-label="Schedule">
      <header className="schedule-toolbar">
        <div className="schedule-toolbar-leading">
          <button
            className="workspace-return-button schedule-app-return"
            type="button"
            onClick={onBack}
          >
            <ArrowLeft aria-hidden="true" />
            <span>Back to App</span>
          </button>
          <div className="schedule-title">
            <CalendarClock aria-hidden="true" />
            <h2>Scheduled tasks</h2>
            <span>{state?.schedules.length ?? 0}</span>
          </div>
        </div>
        <button
          className="secondary-button schedule-new-button"
          type="button"
          onClick={beginNew}
        >
          <Plus aria-hidden="true" />
          <span>New schedule</span>
        </button>
      </header>

      {error && (
        <div className="schedule-notice" role="alert">
          <CircleAlert aria-hidden="true" />
          <span>{error}</span>
          <button type="button" aria-label="Dismiss" title="Dismiss" onClick={() => setError(null)}>
            <X aria-hidden="true" />
          </button>
        </div>
      )}

      <div
        className="schedule-layout"
        data-empty={!editing && (!state || state.schedules.length === 0)}
        data-editing-empty={editing && (!state || state.schedules.length === 0)}
      >
        <div className="schedule-list" aria-label="Scheduled tasks">
          {!state ? (
            <div className="schedule-empty">Loading</div>
          ) : state.schedules.length === 0 ? (
            <div className="schedule-empty">No schedules</div>
          ) : (
            state.schedules.map((schedule) => {
              const run = schedule.runs[schedule.runs.length - 1];
              return (
                <button
                  className={`schedule-list-row ${selectedId === schedule.id ? "active" : ""}`}
                  type="button"
                  key={schedule.id}
                  onClick={() => {
                    setSelectedId(schedule.id);
                    onScheduleSelect(schedule.id);
                    setEditing(false);
                  }}
                >
                  <span className="schedule-list-icon" data-enabled={schedule.enabled}>
                    <Clock3 aria-hidden="true" />
                  </span>
                  <span className="schedule-list-copy">
                    <strong>{schedule.name}</strong>
                    <small>{schedule.projectName} · {schedule.sessionName}</small>
                  </span>
                  <span
                    className="schedule-list-state"
                    data-status={run?.status ?? (schedule.enabled ? "idle" : "paused")}
                  >
                    {run && activeRunStatuses.has(run.status)
                      ? runStatusLabel(run.status)
                      : schedule.enabled
                        ? formatTime(schedule.nextRunAtMs)
                        : "Paused"}
                  </span>
                </button>
              );
            })
          )}
        </div>

        <div className="schedule-detail">
          {editing ? (
            <form
              className="schedule-editor"
              onSubmit={(event) => {
                event.preventDefault();
                void saveDraft();
              }}
            >
              <div className="schedule-editor-heading">
                <h3>{draft.id ? "Edit schedule" : "New schedule"}</h3>
                <button
                  className="icon-button"
                  type="button"
                  aria-label="Close editor"
                  title="Close"
                  onClick={() => setEditing(false)}
                >
                  <X aria-hidden="true" />
                </button>
              </div>

              <label className="schedule-field">
                <span>Name</span>
                <input
                  autoFocus
                  required
                  maxLength={80}
                  value={draft.name}
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, name: event.target.value }))
                  }
                />
              </label>

              <div className="schedule-field-grid">
                <label className="schedule-field">
                  <span>Project</span>
                  <select
                    value={draft.projectId}
                    onChange={(event) => {
                      const projectId = event.target.value;
                      setDraft((current) => ({ ...current, projectId, sessionId: "" }));
                    }}
                  >
                    <option value="">No project</option>
                    {projects.map((project) => (
                      <option value={project.id} key={project.id}>
                        {project.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="schedule-field">
                  <span>Task</span>
                  <select
                    disabled={!draft.projectId}
                    value={draft.sessionId}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, sessionId: event.target.value }))
                    }
                  >
                    <option value="">No task</option>
                    {projectSessions.map((session) => (
                      <option value={session.id} key={session.id}>
                        {session.name}
                      </option>
                    ))}
                  </select>
                </label>
              </div>

              <label className="schedule-field">
                <span>Prompt</span>
                <textarea
                  required
                  rows={6}
                  value={draft.prompt}
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, prompt: event.target.value }))
                  }
                />
              </label>

              <div className="schedule-field schedule-segment-field">
                <span>Repeat</span>
                <div className="schedule-segmented">
                  {cadenceOptions.map((option) => (
                    <button
                      className={draft.cadence === option.value ? "active" : ""}
                      type="button"
                      key={option.value}
                      aria-pressed={draft.cadence === option.value}
                      onClick={() =>
                        setDraft((current) => ({
                          ...current,
                          cadence: option.value,
                          weeklyDays:
                            option.value === "weekly" && current.weeklyDays.length === 0
                              ? [currentWeekday()]
                              : current.weeklyDays
                        }))
                      }
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>

              {draft.cadence === "weekly" && (
                <div className="schedule-field schedule-weekday-field">
                  <span>Runs on</span>
                  <div className="schedule-weekdays" aria-label="Weekly days">
                    {weekdayOptions.map((day) => {
                      const selected = draft.weeklyDays.includes(day.value);
                      return (
                        <button
                          className={selected ? "active" : ""}
                          type="button"
                          key={day.value}
                          aria-pressed={selected}
                          onClick={() =>
                            setDraft((current) => ({
                              ...current,
                              weeklyDays: selected
                                ? current.weeklyDays.filter((value) => value !== day.value)
                                : [...current.weeklyDays, day.value].sort((a, b) => a - b)
                            }))
                          }
                        >
                          {day.label}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              <div className="schedule-field-grid">
                <label className="schedule-field">
                  <span>Starts</span>
                  <input
                    required
                    type="datetime-local"
                    value={draft.startLocal}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, startLocal: event.target.value }))
                    }
                  />
                </label>
                <label className="schedule-field">
                  <span>Ends (optional)</span>
                  <input
                    type="datetime-local"
                    min={draft.startLocal}
                    value={draft.endsLocal}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, endsLocal: event.target.value }))
                    }
                  />
                </label>
              </div>

              <div className="schedule-field schedule-segment-field">
                <span>Effort</span>
                <div className="schedule-segmented schedule-effort-segmented">
                  {effortOptions.map((option) => (
                    <button
                      className={draft.effort === option.value ? "active" : ""}
                      type="button"
                      key={option.value}
                      aria-pressed={draft.effort === option.value}
                      onClick={() =>
                        setDraft((current) => ({ ...current, effort: option.value }))
                      }
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="schedule-switches">
                <label className="settings-switch">
                  <input
                    type="checkbox"
                    checked={draft.enabled}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, enabled: event.target.checked }))
                    }
                  />
                  <span className="settings-switch-track" aria-hidden="true"><span /></span>
                  <span>Enabled</span>
                </label>
                <label className="settings-switch">
                  <input
                    type="checkbox"
                    checked={draft.catchUp}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, catchUp: event.target.checked }))
                    }
                  />
                  <span className="settings-switch-track" aria-hidden="true"><span /></span>
                  <span>Catch up</span>
                </label>
              </div>

              <div className="schedule-editor-actions">
                <button
                  className="secondary-button"
                  type="button"
                  disabled={busyAction === "save"}
                  onClick={() => setEditing(false)}
                >
                  Cancel
                </button>
                <button
                  className="primary-button"
                  type="submit"
                  disabled={
                    busyAction === "save" ||
                    !draft.name.trim() ||
                    !draft.prompt.trim() ||
                    (draft.cadence === "weekly" && draft.weeklyDays.length === 0)
                  }
                >
                  <Save aria-hidden="true" />
                  <span>{busyAction === "save" ? "Saving" : "Save"}</span>
                </button>
              </div>
            </form>
          ) : selected ? (
            <div className="schedule-summary">
              <header className="schedule-summary-header">
                <div>
                  <span className="schedule-eyebrow">{selected.cadence}</span>
                  <h3>{selected.name}</h3>
                  <p>
                    {selected.projectId
                      ? `${selected.projectName} · ${selected.sessionName}`
                      : "Standalone schedule"}
                  </p>
                </div>
                <div className="schedule-summary-actions">
                  {activeRun ? (
                    <button
                      className="icon-button danger"
                      type="button"
                      aria-label="Cancel current run"
                      title="Cancel run"
                      disabled={busyAction === `cancel-${selected.id}`}
                      onClick={() =>
                        void perform(`cancel-${selected.id}`, () =>
                          cancelScheduleRun(selected.id)
                        )
                      }
                    >
                      <XCircle aria-hidden="true" />
                    </button>
                  ) : (
                    <button
                      className="icon-button"
                      type="button"
                      aria-label="Run now"
                      title="Run now"
                      disabled={busyAction === `run-${selected.id}`}
                      onClick={() =>
                        void perform(`run-${selected.id}`, () => runScheduleNow(selected.id))
                      }
                    >
                      <Play aria-hidden="true" />
                    </button>
                  )}
                  <button
                    className="icon-button"
                    type="button"
                    aria-label="Edit schedule"
                    title="Edit"
                    disabled={Boolean(activeRun)}
                    onClick={() => beginEdit(selected)}
                  >
                    <Pencil aria-hidden="true" />
                  </button>
                  <button
                    className="icon-button danger"
                    type="button"
                    aria-label="Delete schedule"
                    title="Delete"
                    disabled={Boolean(activeRun)}
                    onClick={() => setDeleteTarget(selected)}
                  >
                    <Trash2 aria-hidden="true" />
                  </button>
                </div>
              </header>

              <div className="schedule-status-band">
                <div>
                  <span>Next</span>
                  <strong>{formatTime(selected.nextRunAtMs)}</strong>
                </div>
                <div>
                  <span>Ends</span>
                  <strong>{selected.endsAtMs ? formatTime(selected.endsAtMs) : "No end"}</strong>
                </div>
                <div>
                  <span>Effort</span>
                  <strong>{selected.effort}</strong>
                </div>
                <label className="settings-switch schedule-enabled-switch">
                  <input
                    type="checkbox"
                    checked={selected.enabled}
                    disabled={busyAction === `toggle-${selected.id}`}
                    onChange={(event) =>
                      void perform(`toggle-${selected.id}`, () =>
                        setScheduleEnabled(selected.id, event.target.checked)
                      )
                    }
                  />
                  <span className="settings-switch-track" aria-hidden="true"><span /></span>
                  <span>{selected.enabled ? "Enabled" : "Paused"}</span>
                </label>
              </div>

              <div className="schedule-prompt-preview">
                <span>Prompt</span>
                <p>{selected.prompt}</p>
              </div>

              <section className="schedule-history" aria-label="Run history">
                <div className="schedule-history-title">
                  <h4>Run history</h4>
                  {selected.sessionId && (
                    <button
                      className="secondary-button schedule-open-task"
                      type="button"
                      onClick={() => {
                        if (selected.sessionId) onOpenSession(selected.sessionId);
                      }}
                    >
                      <span>Open task</span>
                      <ArrowRight aria-hidden="true" />
                    </button>
                  )}
                </div>
                {selected.runs.length === 0 ? (
                  <div className="schedule-history-empty">No runs</div>
                ) : (
                  [...selected.runs].reverse().map((run) => (
                    <div className="schedule-run-row" key={run.id} data-status={run.status}>
                      {run.status === "completed" ? (
                        <CheckCircle2 aria-hidden="true" />
                      ) : run.status === "failed" || run.status === "cancelled" ? (
                        <CircleAlert aria-hidden="true" />
                      ) : (
                        <Clock3 aria-hidden="true" />
                      )}
                      <span>
                        <strong>{runStatusLabel(run.status)}</strong>
                        <small>{run.source === "manual" ? "Manual" : "Scheduled"}</small>
                      </span>
                      <time>{formatTime(run.scheduledForMs)}</time>
                      {run.error && <p>{run.error}</p>}
                    </div>
                  ))
                )}
              </section>
            </div>
          ) : null}
        </div>
      </div>

      {deleteTarget &&
        createPortal(
          <div className="delete-confirmation-backdrop">
            <section
              className="delete-confirmation-dialog"
              role="alertdialog"
              aria-modal="true"
              aria-labelledby="delete-schedule-title"
              aria-describedby="delete-schedule-description"
            >
              <div className="delete-confirmation-copy">
                <h2 id="delete-schedule-title">Delete “{deleteTarget.name}”?</h2>
                <p id="delete-schedule-description">
                  This removes the schedule and its run history. Conversation history is not affected.
                </p>
              </div>
              <div className="delete-confirmation-actions">
                <button type="button" onClick={() => setDeleteTarget(null)}>
                  Cancel
                </button>
                <button
                  className="danger"
                  type="button"
                  onClick={() => {
                    const scheduleId = deleteTarget.id;
                    setDeleteTarget(null);
                    void perform(`delete-${scheduleId}`, async () => {
                      const next = await deleteSchedule(scheduleId);
                      setSelectedId(next.schedules[0]?.id ?? null);
                      onScheduleSelect(next.schedules[0]?.id ?? null);
                      return next;
                    });
                  }}
                >
                  Delete Schedule
                </button>
              </div>
            </section>
          </div>,
          document.body
        )}
    </section>
  );
}
