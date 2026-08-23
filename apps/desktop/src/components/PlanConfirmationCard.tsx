import { useState } from "react";
import { ListChecks, Play, Trash2, X } from "lucide-react";
import {
  resolveAgentPlanConfirmation,
  type AgentState,
  type PendingPlanConfirmationView,
  type PlanConfirmationDecision
} from "../tauri";

type PlanConfirmationCardProps = {
  sessionId: string;
  confirmation: PendingPlanConfirmationView;
  onResolved: (sessionId: string, next: AgentState) => void;
  onError: (message: string) => void;
};

/**
 * The plan-then-confirm gate: a High/Xhigh run drafted this plan read-only and
 * paused before preparation. The user approves it (the plan joins the run as
 * protected, provenance-stamped context), discards it (ordinary execution),
 * or cancels the run.
 */
export function PlanConfirmationCard({
  sessionId,
  confirmation,
  onResolved,
  onError
}: PlanConfirmationCardProps) {
  const [busy, setBusy] = useState<PlanConfirmationDecision | null>(null);

  async function resolve(decision: PlanConfirmationDecision) {
    if (busy) return;
    setBusy(decision);
    try {
      const next = await resolveAgentPlanConfirmation(sessionId, decision);
      onResolved(sessionId, next);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section
      className="plan-confirmation"
      role="alertdialog"
      aria-labelledby="plan-confirmation-title"
      aria-describedby="plan-confirmation-description"
    >
      <div className="plan-confirmation-header">
        <ListChecks aria-hidden="true" />
        <strong id="plan-confirmation-title">Plan ready for review</strong>
      </div>
      <p id="plan-confirmation-description">
        The agent drafted this plan read-only and has not executed anything yet.
      </p>
      <pre className="plan-confirmation-plan">{confirmation.planMarkdown}</pre>
      <div className="plan-confirmation-actions">
        <button
          className="plan-confirmation-approve"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("approve")}
        >
          <Play aria-hidden="true" />
          <span>Approve and run</span>
        </button>
        <button
          className="plan-confirmation-discard"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("discard")}
        >
          <Trash2 aria-hidden="true" />
          <span>Discard and run</span>
        </button>
        <button
          className="plan-confirmation-cancel"
          type="button"
          disabled={busy !== null}
          onClick={() => void resolve("cancel")}
        >
          <X aria-hidden="true" />
          <span>Cancel run</span>
        </button>
      </div>
    </section>
  );
}
