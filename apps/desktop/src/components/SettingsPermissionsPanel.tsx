import {
  CheckCircle2,
  ChevronDown,
  EyeOff,
  RefreshCw,
  ShieldCheck,
  XCircle
} from "lucide-react";
import type { PermissionReviewItem } from "../tauri";

type PermissionDecision = "allow_once" | "allow_for_session" | "deny";

type SettingsPermissionsPanelProps = {
  activeReviews: PermissionReviewItem[];
  busy: boolean;
  busySessionIds: Set<string>;
  ignoredReviews: PermissionReviewItem[];
  onIgnore: (requestId: string) => void;
  onResolve: (review: PermissionReviewItem, decision: PermissionDecision) => Promise<void>;
  onRestore: (requestId: string) => void;
};

function formatTime(timestampMs: number | null) {
  if (!timestampMs) return "local";
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(timestampMs);
}

function sourceLabel(source: PermissionReviewItem["source"]) {
  if (source === "agent") return "Agent";
  if (source === "browser") return "Browser";
  if (source === "tool") return "Local tool";
  return "System test";
}

function DisclosureChevron() {
  return (
    <span className="settings-disclosure-chevron" aria-hidden="true">
      <ChevronDown />
    </span>
  );
}

export function SettingsPermissionsPanel({
  activeReviews,
  busy,
  busySessionIds,
  ignoredReviews,
  onIgnore,
  onResolve,
  onRestore
}: SettingsPermissionsPanelProps) {
  return (
    <section className="settings-section" data-settings-group="permissions">
      <div className="permission-review-heading">
        <div className="section-title">
          <ShieldCheck size={17} aria-hidden="true" />
          <h2>Pending Reviews</h2>
        </div>
        <span>{activeReviews.length}</span>
      </div>
      <p className="settings-section-copy">
        Review actions that can modify files, run processes, use the network, or access
        sensitive context. Ignored requests remain paused until restored.
      </p>
      {activeReviews.length === 0 ? (
        <div className="permission-review-empty">
          <CheckCircle2 aria-hidden="true" />
          <span>No actions are waiting for review.</span>
        </div>
      ) : (
        <div className="permission-review-list" aria-label="Pending permission reviews">
          {activeReviews.map((review) => {
            const sessionBusy = Boolean(
              review.sessionId && busySessionIds.has(review.sessionId)
            );
            return (
              <article className="permission-review-row" key={review.requestId}>
                <header>
                  <div>
                    <strong>{review.action}</strong>
                    <span>{sourceLabel(review.source)}</span>
                  </div>
                  <em data-risk={review.risk}>{review.risk}</em>
                </header>
                <div className="permission-review-context">
                  <strong title={review.sessionId ?? undefined}>
                    {review.sessionName ?? "No related session"}
                  </strong>
                  <span>
                    {review.projectName ?? "Cindx"} · {formatTime(review.requestedAtMs)}
                  </span>
                </div>
                <p>{review.reason}</p>
                <dl className="permission-review-meta">
                  <div>
                    <dt>Scope</dt>
                    <dd>{review.scope || "Current workspace"}</dd>
                  </div>
                </dl>
                {review.input && (
                  <details className="permission-review-input">
                    <summary>
                      <span>Request details</span>
                      <DisclosureChevron />
                    </summary>
                    <pre>{review.input}</pre>
                  </details>
                )}
                <div className="permission-review-actions">
                  <button
                    className="permission-approve"
                    type="button"
                    disabled={busy || sessionBusy}
                    onClick={() => void onResolve(review, "allow_once")}
                  >
                    <CheckCircle2 aria-hidden="true" />
                    <span>Approve once</span>
                  </button>
                  {review.canAllowSession && (
                    <button
                      className="permission-session"
                      type="button"
                      disabled={busy || sessionBusy}
                      onClick={() => void onResolve(review, "allow_for_session")}
                      title={
                        review.action === "shell.run"
                          ? "Reuse only this exact command in this session"
                          : "Allow this capability for the session"
                      }
                    >
                      <ShieldCheck aria-hidden="true" />
                      <span>
                        {review.action === "shell.run" ? "Allow command" : "Allow session"}
                      </span>
                    </button>
                  )}
                  <button
                    className="permission-deny"
                    type="button"
                    disabled={busy || sessionBusy}
                    onClick={() => void onResolve(review, "deny")}
                  >
                    <XCircle aria-hidden="true" />
                    <span>Reject</span>
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => onIgnore(review.requestId)}
                  >
                    <EyeOff aria-hidden="true" />
                    <span>Ignore</span>
                  </button>
                </div>
              </article>
            );
          })}
        </div>
      )}
      {ignoredReviews.length > 0 && (
        <details className="permission-ignored-reviews">
          <summary>
            <span>Ignored for now ({ignoredReviews.length})</span>
            <DisclosureChevron />
          </summary>
          <div>
            {ignoredReviews.map((review) => (
              <div className="permission-ignored-row" key={review.requestId}>
                <span>
                  <strong>{review.action}</strong>
                  <small>{review.sessionName ?? sourceLabel(review.source)}</small>
                </span>
                <button type="button" onClick={() => onRestore(review.requestId)}>
                  <RefreshCw aria-hidden="true" />
                  <span>Restore</span>
                </button>
              </div>
            ))}
          </div>
        </details>
      )}
    </section>
  );
}
