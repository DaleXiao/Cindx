import { useEffect, useMemo, useRef, useState } from "react";
import { ListChecks, Play, Trash2, X } from "lucide-react";
import {
  resolveAgentPlanConfirmation,
  type AgentState,
  type ApprovalPolicy,
  type PendingPlanConfirmationView
} from "../tauri";
import {
  planAutoApprovalActive,
  planAutoApprovalAllowed,
  planAutoApproveRemainingMs,
  PLAN_AUTO_APPROVAL_WINDOW_MS,
  type PlanConfirmationDecision,
  type PlanResolvedBy
} from "../planModeModel";
import { parsePlanDocument } from "../planConfirmationModel";
import { AgentMarkdown } from "./AgentMarkdown";

type PlanConfirmationCardProps = {
  sessionId: string;
  confirmation: PendingPlanConfirmationView;
  approvalPolicy: ApprovalPolicy;
  onResolved: (sessionId: string, next: AgentState) => void;
  onError: (message: string) => void;
};

/**
 * The plan-then-confirm gate: a High/Xhigh run drafted this plan read-only and
 * paused before preparation. The user approves it (the plan joins the run as
 * protected, provenance-stamped context), discards it (ordinary execution),
 * or cancels the run.
 *
 * Under the strict approval policy only, the card additionally auto-approves
 * after PLAN_AUTO_APPROVAL_WINDOW_MS of no reading/interaction so a run never
 * blocks forever. Any sign of reading (hover, press, focus, scroll), a hidden
 * window, or a backgrounded app pauses the countdown, and the resolution is
 * recorded as `auto-timeout` so audits can tell it from an explicit click.
 */
export function PlanConfirmationCard({
  sessionId,
  confirmation,
  approvalPolicy,
  onResolved,
  onError
}: PlanConfirmationCardProps) {
  const [busy, setBusy] = useState<PlanConfirmationDecision | null>(null);
  // Collapse the card the instant a decision is made so the run visibly hands
  // off to "Agent actions" instead of looking frozen.
  const [dismissed, setDismissed] = useState(false);
  const [engaged, setEngaged] = useState(false);
  const engagedRef = useRef(false);
  const [attention, setAttention] = useState({
    documentVisible: typeof document === "undefined" || !document.hidden,
    windowFocused: typeof document === "undefined" || document.hasFocus()
  });
  // A failed resolution restarts the whole window instead of retrying
  // immediately, so a backend error can never become an approve-per-second
  // loop.
  const [windowStartMs, setWindowStartMs] = useState(() => Date.now());
  const [remainingMs, setRemainingMs] = useState(PLAN_AUTO_APPROVAL_WINDOW_MS);
  const autoApprovalAllowed = planAutoApprovalAllowed(approvalPolicy);

  async function resolve(decision: PlanConfirmationDecision, resolvedBy: PlanResolvedBy) {
    if (busy || dismissed) return;
    setBusy(decision);
    setDismissed(true);
    try {
      const next = await resolveAgentPlanConfirmation(sessionId, decision, resolvedBy);
      onResolved(sessionId, next);
    } catch (error) {
      setDismissed(false);
      setWindowStartMs(Date.now());
      setRemainingMs(PLAN_AUTO_APPROVAL_WINDOW_MS);
      onError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  }

  const resolveRef = useRef(resolve);
  resolveRef.current = resolve;

  // Pause the countdown while the user reads (hover/press/focus/scroll), the
  // window is hidden, or the app is unfocused.
  useEffect(() => {
    const update = () =>
      setAttention({ documentVisible: !document.hidden, windowFocused: document.hasFocus() });
    document.addEventListener("visibilitychange", update);
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    return () => {
      document.removeEventListener("visibilitychange", update);
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
    };
  }, []);

  useEffect(() => {
    if (dismissed || !autoApprovalAllowed) return;
    const timer = setInterval(() => {
      if (
        !planAutoApprovalActive({
          engaged: engagedRef.current,
          documentVisible: attention.documentVisible,
          windowFocused: attention.windowFocused
        })
      ) {
        return;
      }
      const remaining = planAutoApproveRemainingMs(
        confirmation.proposedAtMs,
        windowStartMs,
        Date.now()
      );
      setRemainingMs(remaining);
      if (remaining <= 0) {
        clearInterval(timer);
        void resolveRef.current("approve", "auto-timeout");
      }
    }, 1000);
    return () => clearInterval(timer);
  }, [dismissed, autoApprovalAllowed, attention, windowStartMs, confirmation.proposedAtMs]);

  // Hooks must run before any early return (React #300).
  const planDocument = useMemo(
    () => parsePlanDocument(confirmation.planMarkdown),
    [confirmation.planMarkdown]
  );

  if (dismissed) return null;

  const engage = () => {
    engagedRef.current = true;
    setEngaged(true);
  };

  const countdownSeconds = Math.ceil(remainingMs / 1000);

  const copyCode = (content: string) => {
    void navigator.clipboard
      .writeText(content)
      .catch(() => onError("Could not copy the code block."));
  };

  return (
    <section
      className="plan-confirmation"
      role="alertdialog"
      aria-labelledby="plan-confirmation-title"
      aria-describedby="plan-confirmation-description"
      onPointerDown={engage}
      onPointerEnter={engage}
      onFocus={engage}
    >
      <div className="plan-confirmation-header">
        <ListChecks aria-hidden="true" />
        <strong id="plan-confirmation-title">Plan ready for review</strong>
        {!engaged && autoApprovalAllowed && (
          <span className="plan-confirmation-countdown">auto-approve in {countdownSeconds}s</span>
        )}
      </div>
      <p id="plan-confirmation-description">
        The agent drafted this plan read-only and has not executed anything yet.
      </p>
      <div className="plan-confirmation-plan" onScroll={engage}>
        {planDocument.structured ? (
          <>
            {planDocument.objective && (
              <p className="plan-confirmation-objective">{planDocument.objective}</p>
            )}
            <ol className="plan-confirmation-steps">
              {planDocument.steps.map((step) => (
                <li key={step.index} className="plan-confirmation-step">
                  <span className="plan-confirmation-step-index" aria-hidden="true">
                    {step.index}
                  </span>
                  <div className="plan-confirmation-step-body">
                    <strong>{step.title}</strong>
                    {step.detail && <p>{step.detail}</p>}
                    {step.files.length > 0 && (
                      <span className="plan-confirmation-step-files">
                        {step.files.map((file) => (
                          <code key={file}>{file}</code>
                        ))}
                      </span>
                    )}
                  </div>
                </li>
              ))}
            </ol>
            {planDocument.verification && (
              <p className="plan-confirmation-verification">
                <strong>Verification:</strong> {planDocument.verification}
              </p>
            )}
          </>
        ) : (
          <AgentMarkdown
            content={confirmation.planMarkdown}
            onOpenError={onError}
            onCopyCode={copyCode}
          />
        )}
      </div>
      <div className="plan-confirmation-actions">
        <button
          className="plan-confirmation-approve"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("approve", "local-user")}
        >
          <Play aria-hidden="true" />
          <span>Approve and run</span>
        </button>
        <button
          className="plan-confirmation-discard"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("discard", "local-user")}
        >
          <Trash2 aria-hidden="true" />
          <span>Discard and run</span>
        </button>
        <button
          className="plan-confirmation-cancel"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("cancel", "local-user")}
        >
          <X aria-hidden="true" />
          <span>Cancel run</span>
        </button>
      </div>
    </section>
  );
}
