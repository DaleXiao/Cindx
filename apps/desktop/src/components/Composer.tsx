import {
  Check,
  CheckCheck,
  ChevronDown,
  FileText,
  Image,
  LoaderCircle,
  Plus,
  RotateCcw,
  Send,
  ShieldCheck,
  Square,
  X
} from "lucide-react";
import { useEffect, useRef } from "react";
import type { AgentAttachment, AgentEffort, ToolApprovalView } from "../tauri";

type ComposerProps = {
  value: string;
  working: boolean;
  canStop: boolean;
  canRetry: boolean;
  error: string | null;
  focusRequest: number;
  pendingApproval: ToolApprovalView | null;
  permissionBusy: boolean;
  attachments: AgentAttachment[];
  attachmentBusy: boolean;
  effort: AgentEffort;
  onChange: (value: string) => void;
  onEffortChange: (effort: AgentEffort) => void;
  onSend: (prompt: string) => void;
  onPickAttachments: (files: File[]) => void;
  onRemoveAttachment: (attachment: AgentAttachment) => void;
  onCancel: () => void;
  onRetry: () => void;
  onResolvePermission: (
    requestId: string,
    decision: "allow_once" | "allow_for_session" | "deny"
  ) => void;
};

export function Composer({
  value,
  working,
  canStop,
  canRetry,
  error,
  focusRequest,
  pendingApproval,
  permissionBusy,
  attachments,
  attachmentBusy,
  effort,
  onChange,
  onEffortChange,
  onSend,
  onPickAttachments,
  onRemoveAttachment,
  onCancel,
  onRetry,
  onResolvePermission
}: ComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const composingRef = useRef(false);
  const compositionJustEndedRef = useRef(false);
  const canSend =
    !working && !canStop && !pendingApproval && !attachmentBusy && Boolean(value.trim() || attachments.length);
  const canRetryError = canRetry && Boolean(error) && !working && !canStop && !pendingApproval;
  const effortTitle = {
    fast: "Single model with the lowest latency",
    auto: "Route each request by complexity",
    pro: "Adaptive collaboration with up to three models"
  }[effort];

  useEffect(() => {
    if (focusRequest <= 0 || pendingApproval) return;
    textareaRef.current?.focus();
    textareaRef.current?.setSelectionRange(value.length, value.length);
  }, [focusRequest, pendingApproval, value.length]);

  function submit() {
    if (composingRef.current || compositionJustEndedRef.current) return;
    const prompt = value.trim();
    if (!canSend) return;
    onChange("");
    onSend(prompt);
  }

  return (
    <form
      className="composer"
      data-working={working}
      data-permission={Boolean(pendingApproval)}
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <div className="composer-field">
        {pendingApproval ? (
          <section className="composer-permission" aria-label="Agent permission request">
            <ShieldCheck aria-hidden="true" />
            <div className="composer-permission-copy">
              <strong>{pendingApproval.toolName}</strong>
              <p>{pendingApproval.reason}</p>
              {pendingApproval.scope.trim() && pendingApproval.scope.trim() !== "." && (
                <span title={pendingApproval.scope}>{pendingApproval.scope}</span>
              )}
            </div>
            <div className="composer-permission-actions">
              {pendingApproval.risk !== "destructive" && (
                <button
                  type="button"
                  disabled={permissionBusy}
                  onClick={() =>
                    onResolvePermission(pendingApproval.requestId, "allow_for_session")
                  }
                >
                  <CheckCheck aria-hidden="true" />
                  <span>Allow session</span>
                </button>
              )}
              <button
                type="button"
                disabled={permissionBusy}
                onClick={() => onResolvePermission(pendingApproval.requestId, "allow_once")}
              >
                <Check aria-hidden="true" />
                <span>Once</span>
              </button>
              <button
                type="button"
                disabled={permissionBusy}
                onClick={() => onResolvePermission(pendingApproval.requestId, "deny")}
              >
                <X aria-hidden="true" />
                <span>Deny</span>
              </button>
            </div>
          </section>
        ) : (
          <div className="composer-input-shell">
            {attachments.length > 0 && (
              <div className="composer-attachments" aria-label="Attachments">
                {attachments.map((attachment) => (
                  <div className="composer-attachment" key={attachment.id} title={attachment.path}>
                    {attachment.mimeType.startsWith("image/") ? (
                      <Image aria-hidden="true" />
                    ) : (
                      <FileText aria-hidden="true" />
                    )}
                    <span>{attachment.name}</span>
                    <button
                      type="button"
                      aria-label={`Remove ${attachment.name}`}
                      title="Remove attachment"
                      disabled={working || canStop || attachmentBusy}
                      onClick={() => onRemoveAttachment(attachment)}
                    >
                      <X aria-hidden="true" />
                    </button>
                  </div>
                ))}
              </div>
            )}
            <textarea
              ref={textareaRef}
              value={value}
              onChange={(event) => onChange(event.target.value)}
              onCompositionStart={() => {
                composingRef.current = true;
                compositionJustEndedRef.current = false;
              }}
              onCompositionEnd={(event) => {
                composingRef.current = false;
                compositionJustEndedRef.current = true;
                onChange(event.currentTarget.value);
                // WebKit can emit the candidate-selection Enter after compositionend.
                window.setTimeout(() => {
                  compositionJustEndedRef.current = false;
                }, 0);
              }}
              onKeyDown={(event) => {
                const nativeEvent = event.nativeEvent;
                const imeActive =
                  composingRef.current ||
                  compositionJustEndedRef.current ||
                  nativeEvent.isComposing ||
                  nativeEvent.keyCode === 229;
                if (event.key !== "Enter" || event.shiftKey || imeActive) return;
                if (working || canStop) return;
                event.preventDefault();
                submit();
              }}
              aria-keyshortcuts="Enter"
              placeholder="Message Cindx"
              rows={3}
            />
            <div className="composer-toolbar">
              <input
                ref={fileInputRef}
                className="composer-file-input"
                type="file"
                multiple
                tabIndex={-1}
                onChange={(event) => {
                  const files = Array.from(event.currentTarget.files ?? []);
                  event.currentTarget.value = "";
                  if (files.length > 0) onPickAttachments(files);
                }}
              />
              <button
                className="composer-attach-button"
                type="button"
                aria-label="Attach files"
                title="Attach files"
                disabled={working || canStop || attachmentBusy || attachments.length >= 10}
                onClick={() => fileInputRef.current?.click()}
              >
                {attachmentBusy ? (
                  <LoaderCircle className="composer-attach-loading" aria-hidden="true" />
                ) : (
                  <Plus aria-hidden="true" />
                )}
              </button>
              <div className="composer-toolbar-actions">
                <div className="composer-effort-control">
                  <select
                    className="composer-effort-select"
                    aria-label="Cindx effort"
                    title={effortTitle}
                    value={effort}
                    disabled={working || canStop}
                    onChange={(event) => onEffortChange(event.target.value as AgentEffort)}
                  >
                    <option value="fast">Cindx Fast</option>
                    <option value="auto">Cindx Auto</option>
                    <option value="pro">Cindx Pro</option>
                  </select>
                  <ChevronDown aria-hidden="true" />
                </div>
                <button
                  type={canStop ? "button" : "submit"}
                  className={`send-button composer-primary-button ${canStop ? "stop" : ""}`}
                  aria-label={canStop ? "Stop agent" : "Send message"}
                  title={canStop ? "Stop" : "Send"}
                  disabled={canStop ? false : !canSend}
                  onClick={canStop ? onCancel : undefined}
                >
                  {canStop ? (
                    <span className="composer-stop-icon" data-working={working}>
                      {working && (
                        <LoaderCircle className="composer-working-ring" aria-hidden="true" />
                      )}
                      <Square className="composer-stop-square" aria-hidden="true" />
                    </span>
                  ) : (
                    <Send aria-hidden="true" />
                  )}
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
      {error && (
        <div className="composer-error">
          <span>{error}</span>
          {canRetryError && (
            <button type="button" onClick={onRetry}>
              <RotateCcw aria-hidden="true" />
              <span>Retry</span>
            </button>
          )}
        </div>
      )}
    </form>
  );
}
