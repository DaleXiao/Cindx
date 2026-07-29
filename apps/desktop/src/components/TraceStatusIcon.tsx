import {
  BadgeCheck,
  Ban,
  CheckCircle2,
  CircleDotDashed,
  CircleHelp,
  CirclePause,
  Clock3,
  LoaderCircle,
  OctagonX,
  ShieldQuestion,
  ShieldX
} from "lucide-react";

type TraceStatusIconProps = {
  status: string;
  className?: string;
};

export function TraceStatusIcon({ status, className }: TraceStatusIconProps) {
  const normalized = status.trim().toLowerCase();
  const label = (normalized || "unknown").replace(/_/g, " ");
  let state = "idle";
  let icon = <CircleHelp aria-hidden="true" />;

  if (["done", "completed", "success", "succeeded", "resolved"].includes(normalized)) {
    state = "done";
    icon = <CheckCircle2 aria-hidden="true" />;
  } else if (["allow_once", "allow_for_session", "allowed", "approved"].includes(normalized)) {
    state = "done";
    icon = <BadgeCheck aria-hidden="true" />;
  } else if (["denied", "deny"].includes(normalized)) {
    state = "failed";
    icon = <ShieldX aria-hidden="true" />;
  } else if (["failed", "error"].includes(normalized)) {
    state = "failed";
    icon = <OctagonX aria-hidden="true" />;
  } else if (["cancelled", "canceled"].includes(normalized)) {
    state = "cancelled";
    icon = <Ban aria-hidden="true" />;
  } else if (normalized === "paused") {
    state = "waiting";
    icon = <CirclePause aria-hidden="true" />;
  } else if (normalized === "waiting_for_permission") {
    state = "waiting";
    icon = <ShieldQuestion aria-hidden="true" />;
  } else if (["waiting", "pending"].includes(normalized)) {
    state = "waiting";
    icon = <Clock3 aria-hidden="true" />;
  } else if (["running", "in_progress"].includes(normalized)) {
    state = "running";
    icon = <LoaderCircle aria-hidden="true" />;
  } else if (normalized === "idle") {
    icon = <CircleDotDashed aria-hidden="true" />;
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
