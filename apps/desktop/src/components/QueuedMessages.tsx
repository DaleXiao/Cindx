import {
  Check,
  CornerUpLeft,
  ListEnd,
  MoreHorizontal,
  Paperclip,
  Pencil,
  Trash2,
  X
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { QueuedAgentMessage } from "../tauri";

type QueuedMessagesProps = {
  messages: QueuedAgentMessage[];
  busyId: string | null;
  persistingIds: ReadonlySet<string>;
  onSteer: (queueId: string) => Promise<void>;
  onEdit: (queueId: string, prompt: string) => Promise<void>;
  onDelete: (queueId: string) => Promise<void>;
};

export function QueuedMessages({
  messages,
  busyId,
  persistingIds,
  onSteer,
  onEdit,
  onDelete
}: QueuedMessagesProps) {
  const rootRef = useRef<HTMLElement>(null);
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingPrompt, setEditingPrompt] = useState("");

  useEffect(() => {
    if (!openMenuId) return;
    const closeMenu = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpenMenuId(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenMenuId(null);
    };
    document.addEventListener("pointerdown", closeMenu);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeMenu);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [openMenuId]);

  useEffect(() => {
    if (openMenuId && !messages.some((message) => message.id === openMenuId)) {
      setOpenMenuId(null);
    }
    if (editingId && !messages.some((message) => message.id === editingId)) {
      setEditingId(null);
      setEditingPrompt("");
    }
  }, [editingId, messages, openMenuId]);

  if (messages.length === 0) return null;

  const cancelEdit = () => {
    setEditingId(null);
    setEditingPrompt("");
  };

  const saveEdit = async () => {
    if (!editingId || !editingPrompt.trim() || busyId) return;
    try {
      await onEdit(editingId, editingPrompt.trim());
      cancelEdit();
    } catch {
      // Keep the editor open so the user can retry without losing the draft.
    }
  };

  return (
    <section className="queued-message-stack" aria-label="Queued messages" ref={rootRef}>
      {messages.map((message) => {
        const editing = editingId === message.id;
        const busy = busyId === message.id || persistingIds.has(message.id);
        return (
          <div className="queued-message" data-mode={message.mode} key={message.id}>
            <span
              className="queued-message-mode"
              title={message.mode === "steer" ? "Steer next" : "Queued"}
            >
              {message.mode === "steer" ? (
                <CornerUpLeft aria-hidden="true" />
              ) : (
                <ListEnd aria-hidden="true" />
              )}
            </span>
            {editing ? (
              <input
                className="queued-message-edit"
                aria-label="Edit queued message"
                autoFocus
                value={editingPrompt}
                disabled={busy}
                onChange={(event) => setEditingPrompt(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void saveEdit();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    cancelEdit();
                  }
                }}
              />
            ) : (
              <span className="queued-message-copy" title={message.prompt}>
                {message.prompt}
              </span>
            )}
            {message.attachments.length > 0 && !editing && (
              <span
                className="queued-message-attachments"
                title={`${message.attachments.length} attachment${
                  message.attachments.length === 1 ? "" : "s"
                }`}
              >
                <Paperclip aria-hidden="true" />
                <span>{message.attachments.length}</span>
              </span>
            )}
            {editing ? (
              <div className="queued-message-edit-actions">
                <button
                  type="button"
                  aria-label="Save queued message"
                  title="Save"
                  disabled={busy || !editingPrompt.trim()}
                  onClick={() => void saveEdit()}
                >
                  <Check aria-hidden="true" />
                </button>
                <button
                  type="button"
                  aria-label="Cancel queued message edit"
                  title="Cancel"
                  disabled={busy}
                  onClick={cancelEdit}
                >
                  <X aria-hidden="true" />
                </button>
              </div>
            ) : (
              <div className="queued-message-menu-shell" data-open={openMenuId === message.id}>
                <button
                  className="queued-message-menu-trigger"
                  type="button"
                  aria-label="Queued message actions"
                  aria-haspopup="menu"
                  aria-expanded={openMenuId === message.id}
                  title="Queued message actions"
                  disabled={Boolean(busyId) || persistingIds.has(message.id)}
                  onClick={() =>
                    setOpenMenuId((current) => (current === message.id ? null : message.id))
                  }
                >
                  <MoreHorizontal aria-hidden="true" />
                </button>
                {openMenuId === message.id && (
                  <div className="queued-message-menu" role="menu">
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setOpenMenuId(null);
                        void onSteer(message.id);
                      }}
                    >
                      <CornerUpLeft aria-hidden="true" />
                      <span>Steer</span>
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setOpenMenuId(null);
                        setEditingId(message.id);
                        setEditingPrompt(message.prompt);
                      }}
                    >
                      <Pencil aria-hidden="true" />
                      <span>Edit</span>
                    </button>
                    <button
                      className="danger"
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setOpenMenuId(null);
                        void onDelete(message.id);
                      }}
                    >
                      <Trash2 aria-hidden="true" />
                      <span>Delete</span>
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}
    </section>
  );
}
