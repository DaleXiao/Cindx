import { useRef, useState } from "react";
import {
  BrainCircuit,
  ChevronDown,
  Pin,
  PinOff,
  RefreshCw,
  Save,
  ShieldAlert,
  Trash2
} from "lucide-react";
import {
  partitionProjectMemories,
  projectMemoryActionAllowed,
  summarizeProjectMemories,
  type ProjectMemoryAction,
  type ProjectMemoryItem,
  type ProjectMemoryState
} from "../memoryManagementModel";

export type SettingsMemoryPanelProps = {
  busyMemoryId: string | null;
  error: string | null;
  loading: boolean;
  projectId: string | null;
  refresh: () => Promise<void>;
  state: ProjectMemoryState | null;
  update: (
    item: ProjectMemoryItem,
    action: ProjectMemoryAction,
    confirmedContent?: string
  ) => Promise<boolean>;
};

const memoryTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short"
});

function formatMemoryTime(timestampMs: number) {
  if (!timestampMs) return "Unknown time";
  return memoryTimeFormatter.format(timestampMs);
}

function kindLabel(kind: ProjectMemoryItem["kind"]) {
  if (kind === "requirement") return "Requirement";
  if (kind === "evidence") return "Evidence";
  return "Outcome";
}

function trustLabel(trust: ProjectMemoryItem["trust"]) {
  if (trust === "user_stated") return "User confirmed";
  if (trust === "tool_verified") return "Tool verified";
  if (trust === "assistant_reported") return "Assistant reported";
  return "Legacy — unverified";
}

type MemoryRowProps = {
  busyMemoryId: string | null;
  deleteTargetId: string | null;
  item: ProjectMemoryItem;
  onDeleteTarget: (memoryId: string | null) => void;
  onReview: (item: ProjectMemoryItem) => void;
  onUpdate: SettingsMemoryPanelProps["update"];
  reviewDraft: { memoryId: string; content: string } | null;
  setReviewDraft: (draft: { memoryId: string; content: string } | null) => void;
};

function MemoryRow({
  busyMemoryId,
  deleteTargetId,
  item,
  onDeleteTarget,
  onReview,
  onUpdate,
  reviewDraft,
  setReviewDraft
}: MemoryRowProps) {
  const anyBusy = busyMemoryId !== null;
  const itemBusy = busyMemoryId === item.id;
  const reviewing = reviewDraft?.memoryId === item.id;
  const confirmingDelete = deleteTargetId === item.id;
  const controlId = `memory-${item.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  const enabled = item.state === "active";

  return (
    <article
      className="memory-settings-row"
      data-state={item.state}
      aria-busy={itemBusy || undefined}
    >
      <div className="memory-settings-main">
        <div className="memory-settings-meta">
          <span>{kindLabel(item.kind)}</span>
          <span>{trustLabel(item.trust)}</span>
          {item.state === "active" && item.pinned && (
            <span data-emphasis="true">Pinned</span>
          )}
          {(item.state === "superseded" || item.supersededBy) && <span>Superseded</span>}
        </div>
        <p>{item.content}</p>
        <small title={item.sourceSessionIds.join(", ")}>
          {item.sourceSessionIds.length} source session
          {item.sourceSessionIds.length === 1 ? "" : "s"} · Updated {formatMemoryTime(item.updatedAtMs)}
          {` · ${item.recallCount} recalls · ${item.observedUseCount} observed uses`}
        </small>
        {item.quarantineReason && (
          <small className="memory-settings-reason">{item.quarantineReason}</small>
        )}
      </div>
      <div className="memory-settings-actions">
        {(item.state === "active" || item.state === "disabled") && (
          <label className="settings-switch">
            <input
              type="checkbox"
              checked={enabled}
              disabled={anyBusy}
              aria-label={`${enabled ? "Disable" : "Enable"} memory: ${item.content}`}
              onChange={() => void onUpdate(item, enabled ? "disable" : "enable")}
            />
            <span className="settings-switch-track" aria-hidden="true"><span /></span>
            <span>{enabled ? "On" : "Off"}</span>
          </label>
        )}
        {item.state === "active" && (
          <button
            className="secondary-button"
            type="button"
            disabled={anyBusy}
            aria-pressed={item.pinned}
            title="Pinning changes recall order only; it does not change trust"
            onClick={() => void onUpdate(item, item.pinned ? "unpin" : "pin")}
          >
            {item.pinned ? <PinOff aria-hidden="true" /> : <Pin aria-hidden="true" />}
            <span>{item.pinned ? "Unpin" : "Pin"}</span>
          </button>
        )}
        {item.state === "quarantined" && (
          <button
            className="secondary-button"
            type="button"
            disabled={anyBusy}
            aria-expanded={reviewing}
            aria-controls={`${controlId}-review`}
            onClick={() => onReview(item)}
          >
            <ShieldAlert aria-hidden="true" />
            <span>Review</span>
          </button>
        )}
        <button
          className="secondary-button memory-settings-delete"
          type="button"
          disabled={anyBusy}
          aria-expanded={confirmingDelete}
          aria-controls={`${controlId}-delete`}
          onClick={() => onDeleteTarget(confirmingDelete ? null : item.id)}
        >
          <Trash2 aria-hidden="true" />
          <span>Delete</span>
        </button>
      </div>
      {reviewing && reviewDraft && (
        <div className="memory-settings-review" id={`${controlId}-review`}>
          <label>
            <span>Confirm the durable project requirement</span>
            <textarea
              rows={4}
              maxLength={1200}
              value={reviewDraft.content}
              onChange={(event) =>
                setReviewDraft({ memoryId: item.id, content: event.target.value })
              }
            />
          </label>
          <p>
            Saving records this text as an explicit user-confirmed project requirement.
          </p>
          <div className="button-row">
            <button
              className="secondary-button"
              type="button"
              disabled={anyBusy}
              onClick={() => setReviewDraft(null)}
            >
              Cancel
            </button>
            <button
              className="secondary-button memory-settings-save"
              type="button"
              disabled={anyBusy || !reviewDraft.content.trim()}
              onClick={() => {
                void onUpdate(item, "promote", reviewDraft.content).then((saved) => {
                  if (saved) setReviewDraft(null);
                });
              }}
            >
              <Save aria-hidden="true" />
              <span>Save as project requirement</span>
            </button>
          </div>
        </div>
      )}
      {confirmingDelete && (
        <div
          className="memory-settings-confirm"
          id={`${controlId}-delete`}
          role="group"
          aria-label="Confirm memory deletion"
        >
          <span>Delete permanently? This cannot be undone.</span>
          <button
            className="secondary-button"
            type="button"
            disabled={anyBusy}
            onClick={() => onDeleteTarget(null)}
          >
            Cancel
          </button>
          <button
            className="secondary-button memory-settings-delete-confirm"
            type="button"
            disabled={anyBusy}
            onClick={() => {
              void onUpdate(item, "delete").then((deleted) => {
                if (deleted) onDeleteTarget(null);
              });
            }}
          >
            Delete memory
          </button>
        </div>
      )}
    </article>
  );
}

type MemoryGroupProps = Omit<MemoryRowProps, "item"> & {
  emptyMessage: string;
  items: ProjectMemoryItem[];
  title: string;
};

function MemoryGroup({ emptyMessage, items, title, ...rowProps }: MemoryGroupProps) {
  const contents = items.length === 0
    ? <div className="settings-empty">{emptyMessage}</div>
    : <div className="memory-settings-list">
        {items.map((item) => <MemoryRow key={item.id} item={item} {...rowProps} />)}
      </div>;
  return (
    <details className="memory-settings-group memory-settings-disclosure" aria-label={title}>
      <summary><h3>
        {title}
        <span className="settings-disclosure-chevron" aria-hidden="true"><ChevronDown /></span>
        <span className="memory-settings-group-count">{items.length}</span>
      </h3></summary>
      {contents}
    </details>
  );
}

export function SettingsMemoryPanel({
  busyMemoryId,
  error,
  loading,
  projectId,
  refresh,
  state,
  update
}: SettingsMemoryPanelProps) {
  const refreshButtonRef = useRef<HTMLButtonElement>(null);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [reviewDraft, setReviewDraft] = useState<{
    memoryId: string;
    content: string;
  } | null>(null);
  const visibleState = state?.projectId === projectId ? state : null;
  const sections = partitionProjectMemories(visibleState?.items ?? []);
  const stats = visibleState
    ? summarizeProjectMemories(visibleState.items)
    : {
        records: 0,
        requirements: 0,
        evidence: 0,
        recalls: 0,
        observedUses: 0
      };
  const handleUpdate: SettingsMemoryPanelProps["update"] = async (...args) => {
    const saved = await update(...args);
    if (saved && (args[1] === "delete" || args[1] === "promote")) {
      requestAnimationFrame(() => refreshButtonRef.current?.focus());
    }
    return saved;
  };
  const rowProps = {
    busyMemoryId,
    deleteTargetId,
    onDeleteTarget: (memoryId: string | null) => {
      setDeleteTargetId(memoryId);
      if (memoryId) setReviewDraft(null);
    },
    onReview: (item: ProjectMemoryItem) => {
      setDeleteTargetId(null);
      setReviewDraft({ memoryId: item.id, content: item.content });
    },
    onUpdate: handleUpdate,
    reviewDraft,
    setReviewDraft
  };

  return (
    <section
      className="settings-section memory-settings-panel"
      data-settings-group="knowledge"
      aria-labelledby="project-memory-title"
      aria-busy={loading || undefined}
    >
      <div className="memory-settings-heading">
        <div className="section-title">
          <BrainCircuit aria-hidden="true" />
          <h2 id="project-memory-title">Project memory</h2>
        </div>
        <button
          ref={refreshButtonRef}
          className="secondary-button"
          type="button"
          disabled={loading || busyMemoryId !== null || !projectId}
          onClick={() => void refresh()}
        >
          <RefreshCw className={loading ? "settings-refresh-turn" : undefined} aria-hidden="true" />
          <span>Refresh</span>
        </button>
      </div>
      <p className="settings-section-copy">
        Memory is scoped to this project. Disabled memories stay visible but are not recalled.
        Pinning changes recall order only; it does not change trust.
      </p>
      <div className="rag-stats" aria-label="Project memory stats">
        <div><strong>{stats.records}</strong><span>Memories</span></div>
        <div><strong>{stats.requirements}</strong><span>Requirements</span></div>
        <div><strong>{stats.evidence}</strong><span>Evidence</span></div>
        <div><strong>{stats.recalls} / {stats.observedUses}</strong><span>Recall / use</span></div>
      </div>
      {error && (
        <div className="settings-inline-error memory-settings-error" role="alert">
          <span>{error}</span>
          <button
            className="secondary-button"
            type="button"
            disabled={loading}
            onClick={() => void refresh()}
          >
            Retry
          </button>
        </div>
      )}
      {!projectId ? (
        <div className="settings-empty">Select a project to manage memory.</div>
      ) : loading && !visibleState ? (
        <div className="settings-empty" role="status" aria-live="polite">
          Loading project memory...
        </div>
      ) : visibleState && visibleState.items.length === 0 ? (
        <div className="settings-empty">
          No durable memories for this project yet.
        </div>
      ) : visibleState ? (
        <div className="memory-settings-groups" aria-live="polite">
          <MemoryGroup
            title="Active"
            emptyMessage="No active memories."
            items={sections.active}
            {...rowProps}
          />
          <MemoryGroup
            title="Disabled / inactive"
            emptyMessage="No disabled or superseded memories."
            items={sections.disabled}
            {...rowProps}
          />
          <MemoryGroup
            title="Needs review"
            emptyMessage="No memories need review."
            items={sections.quarantined}
            {...rowProps}
          />
        </div>
      ) : null}
    </section>
  );
}
