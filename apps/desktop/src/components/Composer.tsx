import { Check, CheckCheck, LoaderCircle, Play, Send, ShieldCheck, Square, X } from "lucide-react";
import { useEffect, useRef } from "react";
import type { ToolApprovalView } from "../tauri";

type ComposerProps = {
  value: string;
  working: boolean;
  canStop: boolean;
  canRetry: boolean;
  error: string | null;
  focusRequest: number;
  pendingApproval: ToolApprovalView | null;
  permissionBusy: boolean;
  onChange: (value: string) => void;
  onSend: (prompt: string) => void;
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
  onChange,
  onSend,
  onCancel,
  onRetry,
  onResolvePermission
}: ComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const composingRef = useRef(false);
  const compositionJustEndedRef = useRef(false);
  const canSend = !working && !canStop && !pendingApproval && Boolean(value.trim());
  const retryMode = canRetry && !canSend && !working && !canStop && !pendingApproval;

  useEffect(() => {
    if (focusRequest <= 0 || pendingApproval) return;
    textareaRef.current?.focus();
    textareaRef.current?.setSelectionRange(value.length, value.length);
  }, [focusRequest, pendingApproval, value.length]);

  function submit() {
    if (composingRef.current || compositionJustEndedRef.current) return;
    const prompt = value.trim();
    if (!prompt || !canSend) return;
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
              event.preventDefault();
              submit();
            }}
            disabled={working || canStop}
            aria-keyshortcuts="Enter"
            placeholder="Ask Cindx"
            rows={1}
          />
        )}
      </div>
      {!pendingApproval && (
        <div className="composer-actions">
          <button
            type={canStop || retryMode ? "button" : "submit"}
            className={`send-button composer-primary-button ${canStop ? "stop" : ""}`}
            aria-label={canStop ? "Stop agent" : retryMode ? "Retry agent task" : "Send message"}
            title={canStop ? "Stop" : retryMode ? "Retry" : "Send"}
            disabled={canStop || retryMode ? false : !canSend}
            onClick={canStop ? onCancel : retryMode ? onRetry : undefined}
          >
            {canStop ? (
              <span className="composer-stop-icon" data-working={working}>
                {working && <LoaderCircle className="composer-working-ring" aria-hidden="true" />}
                <Square className="composer-stop-square" aria-hidden="true" />
              </span>
            ) : retryMode ? (
              <Play aria-hidden="true" />
            ) : (
              <Send aria-hidden="true" />
            )}
          </button>
        </div>
      )}
      {error && <div className="composer-error">{error}</div>}
    </form>
  );
}
