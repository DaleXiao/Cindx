import {
  Check,
  CheckCheck,
  ChevronUp,
  CircleCheck,
  FileText,
  Image,
  LoaderCircle,
  Plus,
  RotateCcw,
  Send,
  Settings2,
  ShieldCheck,
  Square,
  X
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useArtifactImagePreview } from "../controllers/useArtifactImagePreview";
import type { AgentAttachment, AgentEffort, ToolApprovalView } from "../tauri";
import type { VoiceInputStatus } from "../voice/voiceInputModel";
import type { ProviderVoiceTransport } from "../providerProfiles";
import {
  providerSubmissionPreflight,
  type ProviderReadiness
} from "../providerReadinessModel";
import { applyCustomCommandTemplate } from "../customCommandsModel";
import { composerTextareaSizing } from "../composerSizingModel";
import type { CustomCommandView } from "../tauriTypes";
import { permissionFocusTarget } from "./accessibilityFocusModel";
import { CustomCommandsMenu } from "./CustomCommandsMenu";
import { VoiceInputButton } from "./VoiceInputButton";
import { WorkspaceUndoControl } from "./WorkspaceUndoControl";

const COMPOSER_TEXTAREA_MIN_HEIGHT = 58;
const COMPOSER_TEXTAREA_MAX_HEIGHT = 180;
const IME_POST_COMPOSITION_ENTER_GUARD_MS = 120;
const SHELL_PERMISSION_PREVIEW_CHARS = 2_000;

function approvalInputSummary(approval: ToolApprovalView | null) {
  if (approval?.toolName !== "shell.run" || !approval.input.trim()) return null;
  let command = approval.input;
  try {
    const input = JSON.parse(approval.input) as { command?: unknown };
    if (typeof input.command === "string") command = input.command;
  } catch {}
  if (command.length <= SHELL_PERMISSION_PREVIEW_CHARS) return command;
  const tailLength = Math.floor(SHELL_PERMISSION_PREVIEW_CHARS / 4);
  return `${command.slice(0, SHELL_PERMISSION_PREVIEW_CHARS - tailLength)}…${command.slice(
    -tailLength
  )}`;
}

const EFFORT_OPTIONS: Array<{
  value: AgentEffort;
  label: string;
  description: string;
}> = [
  {
    value: "fast",
    label: "Cindx Fast",
    description: "Quick direct answer; lowest latency"
  },
  {
    value: "auto",
    label: "Cindx Auto",
    description: "Adaptive execution with independent delivery verification"
  },
  {
    value: "pro",
    label: "Cindx Pro",
    description: "Deep iterative execution with the largest budget"
  }
];

function ComposerAttachmentPreview({ attachment }: { attachment: AgentAttachment }) {
  const isImage = attachment.mimeType.startsWith("image/");
  const dataUrl = useArtifactImagePreview(isImage ? attachment.path : null);

  if (dataUrl) {
    return (
      <img
        className="composer-attachment-preview"
        src={dataUrl}
        alt=""
        aria-hidden="true"
      />
    );
  }
  return isImage ? <Image aria-hidden="true" /> : <FileText aria-hidden="true" />;
}

type ComposerProps = {
  value: string;
  working: boolean;
  canStop: boolean;
  canRetry: boolean;
  canContinue: boolean;
  error: string | null;
  focusRequest: number;
  pendingApproval: ToolApprovalView | null;
  permissionBusy: boolean;
  attachments: AgentAttachment[];
  attachmentBusy: boolean;
  effort: AgentEffort;
  sessionId: string | null;
  voiceConfigured: boolean;
  voiceTransport: ProviderVoiceTransport;
  providerReadiness: ProviderReadiness;
  onChange: (value: string) => void;
  onEffortChange: (effort: AgentEffort) => void;
  onVoiceTranscript: (sessionId: string, text: string) => void;
  onVoiceError: (message: string) => void;
  onProviderRequired: (readiness: ProviderReadiness) => void;
  onConfigureProvider: () => void;
  onSend: (prompt: string) => void;
  onPickAttachments: (files: File[]) => void;
  onRemoveAttachment: (attachment: AgentAttachment) => void;
  onCancel: () => void;
  onRetry: () => void;
  onDismissError: () => void;
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
  canContinue,
  error,
  focusRequest,
  pendingApproval,
  permissionBusy,
  attachments,
  attachmentBusy,
  effort,
  sessionId,
  voiceConfigured,
  voiceTransport,
  providerReadiness,
  onChange,
  onEffortChange,
  onVoiceTranscript,
  onVoiceError,
  onProviderRequired,
  onConfigureProvider,
  onSend,
  onPickAttachments,
  onRemoveAttachment,
  onCancel,
  onRetry,
  onDismissError,
  onResolvePermission
}: ComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const permissionDialogRef = useRef<HTMLElement>(null);
  const previousApprovalRequestIdRef = useRef<string | null>(null);
  const restoreComposerAfterPermissionRef = useRef<{
    requestId: string;
    sessionId: string | null;
  } | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const effortControlRef = useRef<HTMLDivElement>(null);
  const effortTriggerRef = useRef<HTMLButtonElement>(null);
  const composingRef = useRef(false);
  const imeEnterSeenDuringCompositionRef = useRef(false);
  const suppressImeEnterUntilRef = useRef(0);
  const [effortMenuOpen, setEffortMenuOpen] = useState(false);
  const [voiceStatus, setVoiceStatus] = useState<VoiceInputStatus>("idle");
  const hasInput = Boolean(value.trim() || attachments.length);
  const agentActive = working || canStop;
  const showStop = agentActive && !hasInput;
  const voiceBusy = voiceStatus !== "idle";
  const canSend = !pendingApproval && !attachmentBusy && !voiceBusy && hasInput;
  const canRetryError = canRetry && Boolean(error) && !working && !canStop && !pendingApproval;
  const canContinueRun = canContinue && !working && !canStop && !pendingApproval;
  const providerPreflight = providerSubmissionPreflight(providerReadiness);
  const activeEffort = EFFORT_OPTIONS.find((option) => option.value === effort)!;
  const approvalInput = approvalInputSummary(pendingApproval);

  useEffect(() => {
    if (pendingApproval) setVoiceStatus("idle");
  }, [pendingApproval]);

  useLayoutEffect(() => {
    const currentRequestId = pendingApproval?.requestId ?? null;
    const previousRequestId = previousApprovalRequestIdRef.current;
    const restoreRequest = restoreComposerAfterPermissionRef.current;
    const focusTarget = permissionFocusTarget(
      previousRequestId,
      currentRequestId,
      Boolean(
        previousRequestId &&
          restoreRequest?.requestId === previousRequestId &&
          restoreRequest.sessionId === sessionId
      )
    );
    previousApprovalRequestIdRef.current = currentRequestId;
    if (currentRequestId !== previousRequestId) {
      restoreComposerAfterPermissionRef.current = null;
    }
    if (focusTarget === "request") {
      permissionDialogRef.current?.focus({ preventScroll: true });
    } else if (focusTarget === "composer") {
      textareaRef.current?.focus({ preventScroll: true });
      textareaRef.current?.setSelectionRange(value.length, value.length);
    }
  }, [pendingApproval?.requestId, sessionId]);

  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = `${COMPOSER_TEXTAREA_MIN_HEIGHT}px`;
    const sizing = composerTextareaSizing(
      textarea.scrollHeight,
      COMPOSER_TEXTAREA_MIN_HEIGHT,
      COMPOSER_TEXTAREA_MAX_HEIGHT
    );
    textarea.style.height = `${sizing.heightPx}px`;
    textarea.style.overflowY = sizing.overflowY;
  }, [value]);

  useEffect(() => {
    if (!effortMenuOpen) return;
    const closeOnPointerDown = (event: globalThis.PointerEvent) => {
      if (!effortControlRef.current?.contains(event.target as Node)) {
        setEffortMenuOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setEffortMenuOpen(false);
      effortTriggerRef.current?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [effortMenuOpen]);

  useEffect(() => {
    if (working || canStop) setEffortMenuOpen(false);
  }, [canStop, working]);

  useEffect(() => {
    if (focusRequest <= 0 || pendingApproval) return;
    textareaRef.current?.focus();
    textareaRef.current?.setSelectionRange(value.length, value.length);
  }, [focusRequest, pendingApproval, value.length]);

  function submit() {
    if (composingRef.current || voiceBusy) return;
    const prompt = (textareaRef.current?.value ?? value).trim();
    if (pendingApproval || attachmentBusy || (!prompt && attachments.length === 0)) return;
    if (!providerPreflight.allowSubmit) {
      onProviderRequired(providerReadiness);
      return;
    }
    onSend(prompt);
    if (providerPreflight.clearDraft) onChange("");
  }

  function resolvePendingPermission(
    decision: "allow_once" | "allow_for_session" | "deny"
  ) {
    if (!pendingApproval) return;
    restoreComposerAfterPermissionRef.current = {
      requestId: pendingApproval.requestId,
      sessionId
    };
    onResolvePermission(pendingApproval.requestId, decision);
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
          <section
            key={pendingApproval.requestId}
            className="composer-permission"
            role="alertdialog"
            aria-labelledby="composer-permission-title"
            aria-describedby="composer-permission-description"
            ref={permissionDialogRef}
            tabIndex={-1}
          >
            <ShieldCheck aria-hidden="true" />
            <div className="composer-permission-copy">
              <strong id="composer-permission-title">{pendingApproval.toolName}</strong>
              <p id="composer-permission-description">{pendingApproval.reason}</p>
              {approvalInput && (
                <code className="composer-permission-input" title={approvalInput}>
                  {approvalInput}
                </code>
              )}
              {pendingApproval.scope.trim() && pendingApproval.scope.trim() !== "." && (
                <span title={pendingApproval.scope}>{pendingApproval.scope}</span>
              )}
            </div>
            <div className="composer-permission-actions">
              <button
                className="permission-once"
                type="button"
                disabled={permissionBusy}
                onClick={() => resolvePendingPermission("allow_once")}
              >
                <CircleCheck aria-hidden="true" />
                <span>Once</span>
              </button>
              {pendingApproval.canAllowSession && (
                <button
                  className="permission-session"
                  type="button"
                  disabled={permissionBusy}
                  onClick={() => resolvePendingPermission("allow_for_session")}
                  title={
                    pendingApproval.toolName === "shell.run"
                      ? "Reuse only this exact command in this session"
                      : "Allow this capability for the session"
                  }
                >
                  <CheckCheck aria-hidden="true" />
                  <span>
                    {pendingApproval.toolName === "shell.run"
                      ? "Allow command"
                      : "Allow session"}
                  </span>
                </button>
              )}
              <button
                className="permission-deny"
                type="button"
                disabled={permissionBusy}
                onClick={() => resolvePendingPermission("deny")}
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
                  <div
                    className="composer-attachment"
                    data-image={attachment.mimeType.startsWith("image/")}
                    key={attachment.id}
                    title={attachment.path}
                  >
                    <ComposerAttachmentPreview attachment={attachment} />
                    <span>{attachment.name}</span>
                    <button
                      type="button"
                      aria-label={`Remove ${attachment.name}`}
                      title="Remove attachment"
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
              onPaste={(event) => {
                if (attachmentBusy || attachments.length >= 10) return;
                const pastedImages = Array.from(event.clipboardData.items)
                  .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
                  .map((item, index) => {
                    const file = item.getAsFile();
                    if (!file) return null;
                    const extension = file.type.split("/")[1]?.replace("jpeg", "jpg") || "png";
                    const name = file.name.trim() || `pasted-image-${Date.now()}-${index + 1}.${extension}`;
                    return file.name.trim()
                      ? file
                      : new File([file], name, { type: file.type, lastModified: file.lastModified });
                  })
                  .filter((file): file is File => Boolean(file));
                if (pastedImages.length === 0) return;
                event.preventDefault();
                onPickAttachments(pastedImages);
              }}
              onCompositionStart={() => {
                composingRef.current = true;
                imeEnterSeenDuringCompositionRef.current = false;
                suppressImeEnterUntilRef.current = 0;
              }}
              onCompositionEnd={(event) => {
                composingRef.current = false;
                onChange(event.currentTarget.value);
                suppressImeEnterUntilRef.current = imeEnterSeenDuringCompositionRef.current
                  ? 0
                  : performance.now() + IME_POST_COMPOSITION_ENTER_GUARD_MS;
                imeEnterSeenDuringCompositionRef.current = false;
              }}
              onKeyDown={(event) => {
                const nativeEvent = event.nativeEvent;
                if (event.key !== "Enter") {
                  if (!composingRef.current) suppressImeEnterUntilRef.current = 0;
                  return;
                }
                if (event.shiftKey) {
                  suppressImeEnterUntilRef.current = 0;
                  return;
                }
                const imeComposing =
                  composingRef.current ||
                  nativeEvent.isComposing ||
                  nativeEvent.keyCode === 229;
                if (imeComposing) {
                  imeEnterSeenDuringCompositionRef.current = true;
                  return;
                }
                if (performance.now() < suppressImeEnterUntilRef.current) {
                  event.preventDefault();
                  suppressImeEnterUntilRef.current = 0;
                  return;
                }
                event.preventDefault();
                submit();
              }}
              aria-keyshortcuts="Enter"
              placeholder="Message Cindx"
              rows={1}
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
                disabled={attachmentBusy || attachments.length >= 10}
                onClick={() => fileInputRef.current?.click()}
              >
                {attachmentBusy ? (
                  <LoaderCircle className="composer-attach-loading" aria-hidden="true" />
                ) : (
                  <Plus aria-hidden="true" />
                )}
              </button>
              <div className="composer-toolbar-actions">
                <CustomCommandsMenu
                  disabled={working || canStop}
                  onApply={(command: CustomCommandView) => {
                    if (command.effort === "fast" || command.effort === "auto" || command.effort === "pro") {
                      onEffortChange(command.effort);
                    }
                    onChange(applyCustomCommandTemplate(command.template, value));
                  }}
                />
                <div
                  className="composer-effort-control"
                  data-open={effortMenuOpen}
                  ref={effortControlRef}
                >
                  <button
                    className="composer-effort-trigger"
                    type="button"
                    aria-label={`Effort: ${activeEffort.label}`}
                    aria-haspopup="listbox"
                    aria-expanded={effortMenuOpen}
                    title={activeEffort.description}
                    disabled={working || canStop}
                    ref={effortTriggerRef}
                    onClick={() => setEffortMenuOpen((current) => !current)}
                  >
                    <span>{activeEffort.label}</span>
                    <ChevronUp aria-hidden="true" />
                  </button>
                  {effortMenuOpen && (
                    <div className="composer-effort-menu" role="listbox" aria-label="Cindx effort">
                      {EFFORT_OPTIONS.map((option) => (
                        <button
                          type="button"
                          role="option"
                          aria-selected={option.value === effort}
                          data-selected={option.value === effort}
                          key={option.value}
                          onClick={(event) => {
                            const restoreKeyboardFocus = event.detail === 0;
                            onEffortChange(option.value);
                            setEffortMenuOpen(false);
                            if (restoreKeyboardFocus) {
                              window.requestAnimationFrame(() =>
                                effortTriggerRef.current?.focus({ preventScroll: true })
                              );
                            }
                          }}
                        >
                          <span>
                            <strong>{option.label}</strong>
                            <small>{option.description}</small>
                          </span>
                          {option.value === effort && <Check aria-hidden="true" />}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
                <VoiceInputButton
                  configured={voiceConfigured}
                  transport={voiceTransport}
                  sessionId={sessionId}
                  onTranscript={onVoiceTranscript}
                  onError={onVoiceError}
                  onStatusChange={setVoiceStatus}
                />
                <button
                  type={showStop ? "button" : "submit"}
                  className="send-button composer-primary-button"
                  data-mode={showStop ? "stop" : "send"}
                  aria-label={showStop ? "Stop agent" : agentActive ? "Queue message" : "Send message"}
                  title={showStop ? "Stop" : agentActive ? "Queue" : "Send"}
                  disabled={showStop ? !canStop : !canSend}
                  onClick={showStop ? onCancel : undefined}
                >
                  {showStop ? (
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
        <div className="composer-error" role="alert" aria-live="assertive" aria-atomic="true">
          <span>{error}</span>
          <div className="composer-error-actions">
            {canRetryError && (
              <button type="button" onClick={onRetry}>
                <RotateCcw aria-hidden="true" />
                <span>Retry</span>
              </button>
            )}
            {providerPreflight.showModelsCta && (
              <button type="button" onClick={onConfigureProvider}>
                <Settings2 aria-hidden="true" />
                <span>Configure Models</span>
              </button>
            )}
            <button
              className="composer-error-dismiss"
              type="button"
              aria-label="Dismiss error"
              title="Dismiss"
              onClick={onDismissError}
            >
              <X aria-hidden="true" />
            </button>
          </div>
        </div>
      )}
      {!error && canContinueRun && (
        <div className="composer-continuation">
          <span>Run paused at a safety checkpoint.</span>
          <button type="button" onClick={onRetry}>
            <RotateCcw aria-hidden="true" />
            <span>Continue</span>
          </button>
        </div>
      )}
      <WorkspaceUndoControl sessionId={sessionId} disabled={working} />
    </form>
  );
}
