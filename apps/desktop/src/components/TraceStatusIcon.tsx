import {
  Activity,
  CheckCircle2,
  Clock3,
  ShieldQuestion,
  XCircle
} from "lucide-react";

type TraceStatusIconProps = {
  status: string;
  className?: string;
};

export function TraceStatusIcon({ status, className }: TraceStatusIconProps) {
  const normalized = status.trim().toLowerCase();
  const label = (normalized || "unknown").replace(/_/g, " ");
  let state = "idle";
  let icon = <Clock3 aria-hidden="true" />;

  if (["done", "completed", "success", "succeeded", "resolved"].includes(normalized)) {
    state = "done";
    icon = <CheckCircle2 aria-hidden="true" />;
  } else if (["failed", "error"].includes(normalized)) {
    state = "failed";
    icon = <XCircle aria-hidden="true" />;
  } else if (["cancelled", "canceled"].includes(normalized)) {
    state = "cancelled";
    icon = <XCircle aria-hidden="true" />;
  } else if (normalized === "waiting_for_permission") {
    state = "waiting";
    icon = <ShieldQuestion aria-hidden="true" />;
  } else if (["waiting", "pending"].includes(normalized)) {
    state = "waiting";
    icon = <Clock3 aria-hidden="true" />;
  } else if (["running", "in_progress"].includes(normalized)) {
    state = "running";
    icon = <Activity aria-hidden="true" />;
  }

  return (
    <span
      className={`trace-status-icon${className ? ` ${className}` : ""}`}
      data-state={state}
      role="img"
      aria-label={`Status: ${label}`}
      title={label}
    >
      {icon}
    </span>
  );
}
