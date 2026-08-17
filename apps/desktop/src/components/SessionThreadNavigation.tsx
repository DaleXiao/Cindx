import { ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { memo, type RefObject } from "react";
import type { TimelineEntry } from "../tauri";
import type { ThreadRow } from "./sessionThreadProjection";

export const MIN_MINIMAP_MARKERS = 2;
export const MAX_MINIMAP_MARKERS = 32;
export const MINIMAP_MARKER_GAP = 12;
export const LATEST_OUTPUT_THRESHOLD = 48;

const threadTimeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23"
});

export function formatThreadTime(timestampMs: number) {
  return Number.isFinite(timestampMs) ? threadTimeFormatter.format(timestampMs) : "";
}

type ThreadFindProps = {
  open: boolean;
  query: string;
  currentIndex: number;
  matchCount: number;
  inputRef: RefObject<HTMLInputElement>;
  onQueryChange: (query: string) => void;
  onMove: (direction: number) => void;
  onClose: () => void;
};

export function ThreadFind({
  open,
  query,
  currentIndex,
  matchCount,
  inputRef,
  onQueryChange,
  onMove,
  onClose
}: ThreadFindProps) {
  if (!open) return null;
  return (
    <div className="thread-find" role="search">
      <Search aria-hidden="true" />
      <input
        ref={inputRef}
        value={query}
        aria-label="Find in current session"
        placeholder="Find in session"
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          }
          if (event.key === "Enter") {
            event.preventDefault();
            onMove(event.shiftKey ? -1 : 1);
          }
        }}
      />
      <span aria-live="polite">
        {query ? `${matchCount === 0 ? 0 : currentIndex + 1}/${matchCount}` : ""}
      </span>
      <button
        type="button"
        disabled={matchCount === 0}
        aria-label="Previous match"
        title="Previous match"
        onClick={() => onMove(-1)}
      >
        <ChevronUp aria-hidden="true" />
      </button>
      <button
        type="button"
        disabled={matchCount === 0}
        aria-label="Next match"
        title="Next match"
        onClick={() => onMove(1)}
      >
        <ChevronDown aria-hidden="true" />
      </button>
      <button type="button" aria-label="Close find" title="Close" onClick={onClose}>
        <X aria-hidden="true" />
      </button>
    </div>
  );
}

export function minimapMarkerPosition(index: number, markerCount: number) {
  const centerIndex = Math.max(0, markerCount - 1) / 2;
  const centerOffset = (index - centerIndex) * MINIMAP_MARKER_GAP;
  if (centerOffset === 0) return "50%";
  return `calc(50% ${centerOffset < 0 ? "-" : "+"} ${Math.abs(centerOffset)}px)`;
}

export function activeRunProgress(timeline: TimelineEntry[], runStartedAtMs: number) {
  let startIndex = -1;
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const event = timeline[index];
    if (event.label === "Status" && /agent task started/i.test(event.detail)) {
      startIndex = index;
      break;
    }
  }
  const timelineStartedAtMs = startIndex >= 0 ? timeline[startIndex].timestampMs : 0;
  const startedAtMs = runStartedAtMs || timelineStartedAtMs || Date.now();
  let latest: TimelineEntry | undefined;
  let candidateStarts = 0;
  let candidateFinishes = 0;
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const event = timeline[index];
    if (event.timestampMs < startedAtMs) break;
    if (/^Approach \d+$/.test(event.label)) {
      if (/started/i.test(event.detail)) candidateStarts += 1;
      if (/finished/i.test(event.detail)) candidateFinishes += 1;
    }
    if (!latest && event.label !== "Message" && !/agent router selected/i.test(event.detail)) {
      latest = event;
    }
  }
  if (!latest) {
    return {
      label: "Thinking",
      detail: "Cindx is working"
    };
  }

  let label = "Thinking";
  if (latest.label === "Tool started") {
    label = latest.detail.replace(/^Executing\s+/i, "Running ");
  } else if (latest.label === "Tool proposed") {
    label = "Preparing tool call";
  } else if (latest.label === "Tool finished") {
    label = "Processing tool result";
  } else if (latest.label === "Permission requested") {
    label = "Waiting for approval";
  } else if (latest.label === "Permission resolved") {
    label = "Resuming after approval";
  } else if (/preparing workspace knowledge/i.test(latest.detail)) {
    label = "Searching workspace knowledge";
  } else if (/preparing execution strategy/i.test(latest.detail)) {
    label = "Planning work";
  } else if (/starting execution/i.test(latest.detail)) {
    label = "Executing plan";
  } else if (/^Approach \d+$/.test(latest.label)) {
    label = `Exploring approaches ${Math.min(candidateFinishes, candidateStarts)}/${Math.max(
      1,
      candidateStarts
    )}`;
  } else if (latest.label === "Planning" || latest.label === "Planning repair") {
    label = "Planning work";
  } else if (latest.label === "Selection") {
    label = "Selecting approach";
  } else if (latest.label === "Execution" || latest.label === "Model started") {
    label = "Executing plan";
  } else if (latest.label === "Review") {
    label = "Reviewing result";
  } else if (latest.label === "Synthesis") {
    label = "Writing final response";
  }

  return { label, detail: latest.detail };
}

export const RunProgressStatus = memo(function RunProgressStatus({
  progress,
  className
}: {
  progress: ReturnType<typeof activeRunProgress>;
  className: string;
}) {
  return (
    <div className={`thread-thinking ${className}`} role="status">
      <span title={progress.detail}>{progress.label}</span>
    </div>
  );
});

export function estimateThreadRowSize(row: ThreadRow) {
  if (row.type === "tool-chain") return 54;
  if (row.item.type === "event" || row.item.message.role === "tool") return 54;
  const content = row.item.message.content;
  const explicitLines = Math.max(1, content.split("\n").length);
  const wrappedLines = Math.max(1, Math.ceil(content.length / 72));
  const lineCount = Math.max(explicitLines, wrappedLines);
  if (row.item.message.role === "user") {
    const attachmentRows = Math.ceil((row.item.message.attachments?.length ?? 0) / 2);
    return 64 + Math.min(8, lineCount) * 18 + attachmentRows * 148;
  }
  return 48 + Math.min(48, lineCount) * 20;
}
