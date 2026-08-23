import {
  BrainCircuit,
  ChevronRight,
  MessageSquareText,
  ShieldQuestion
} from "lucide-react";
import { memo, useEffect, useState } from "react";
import { ThinkingOrb } from "thinking-orbs";
import type { TimelineEntry } from "../tauri";
import { ToolActivityIcon } from "./ToolActivityIcon";
import { TraceStatusIcon } from "./TraceStatusIcon";
import {
  isToolRequestPlaceholder,
  type SessionThreadSelection,
  type ThreadRow
} from "./sessionThreadProjection";

function usePrefersReducedMotion() {
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(
    () =>
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const handleChange = (event: MediaQueryListEvent) => setPrefersReducedMotion(event.matches);
    setPrefersReducedMotion(query.matches);
    query.addEventListener("change", handleChange);
    return () => query.removeEventListener("change", handleChange);
  }, []);

  return prefersReducedMotion;
}

function AgentActionOrb() {
  const prefersReducedMotion = usePrefersReducedMotion();
  return (
    <ThinkingOrb
      className="thread-agent-action-orb"
      state="shaping"
      size={20}
      paused={prefersReducedMotion}
      aria-hidden="true"
    />
  );
}

export function EventIcon({ event }: { event: TimelineEntry }) {
  if (event.kind === "tool") {
    return (
      <ToolActivityIcon
        toolName={event.toolName ?? (event.label === "Retrieval" ? "semantic_rag" : null)}
      />
    );
  }
  if (event.kind === "permission") return <ShieldQuestion aria-hidden="true" />;
  if (event.kind === "model") return <BrainCircuit aria-hidden="true" />;
  return <MessageSquareText aria-hidden="true" />;
}

export function toolMessageSummary(content: string) {
  const tool = content.match(/^tool=(.+)$/m)?.[1]?.trim();
  const status = content.match(/^status=(.+)$/m)?.[1]?.trim();
  return { label: tool || "Tool output", status: status || "done" };
}

function toolMessageDetail(content: string) {
  return (
    content
      .split("\n")
      .map((line) => line.trim())
      .find((line) => line && !/^(tool|status)=/i.test(line)) ?? "Tool output"
  );
}

function ToolChainItem({
  item,
  selected,
  onSelect
}: {
  item: SessionThreadSelection;
  selected: boolean;
  onSelect: (selection: SessionThreadSelection) => void;
}) {
  if (isToolRequestPlaceholder(item)) return null;

  if (item.type === "event") {
    return (
      <button
        className={`thread-tool-chain-item ${selected ? "selected" : ""}`}
        type="button"
        onClick={() => onSelect(item)}
      >
        <span className="thread-event-icon">
          <EventIcon event={item.event} />
        </span>
        <span className="thread-tool-chain-copy">
          <strong>{item.event.label}</strong>
          <small>{item.event.detail}</small>
        </span>
        <span className="thread-event-meta">
          <TraceStatusIcon status={item.event.state} />
        </span>
      </button>
    );
  }

  const summary = toolMessageSummary(item.message.content);
  return (
    <button
      className={`thread-tool-chain-item ${selected ? "selected" : ""}`}
      type="button"
      onClick={() => onSelect(item)}
    >
      <ToolActivityIcon toolName={summary.label} />
      <span className="thread-tool-chain-copy">
        <strong>{summary.label}</strong>
        <small>{toolMessageDetail(item.message.content)}</small>
      </span>
      <span className="thread-event-meta">
        <TraceStatusIcon status={summary.status} />
      </span>
    </button>
  );
}

export const ToolChainDisclosure = memo(function ToolChainDisclosure({
  row,
  selectedId,
  onSelect,
  active = false,
  progressLabel, progressDetail,
  subagents
}: {
  row: Extract<ThreadRow, { type: "tool-chain" }>;
  selectedId: string | null;
  onSelect: (selection: SessionThreadSelection) => void;
  active?: boolean;
  progressLabel?: string; progressDetail?: string;
  subagents?: { description: string; done: boolean }[];
}) {
  const [open, setOpen] = useState(false);
  const selected = row.items.some((item) => item.id === selectedId);
  const liveSubagents = (subagents ?? []).some((subagent) => !subagent.done)
    ? subagents ?? []
    : [];

  return (
    <details
      className={`thread-tool-chain ${selected ? "selected" : ""}`}
      data-minimap-id={row.id}
      data-minimap-index={row.itemIndex}
      data-minimap-kind="tool-chain"
      data-active={active || undefined}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <strong>Agent actions</strong>
        {progressLabel ? (
          <span className="thread-tool-chain-status" title={progressDetail}>{progressLabel}</span>
        ) : null}
        <ChevronRight className="thread-tool-chain-chevron" aria-hidden="true" />
      </summary>
      {liveSubagents.length > 0 && (
        <div className="thread-tool-chain-subagents">
          {liveSubagents.map((subagent) => (
            <div className="thread-tool-chain-subagent" key={subagent.description}>
              <span className="thread-tool-chain-subagent-icon">
                {subagent.done ? (
                  <TraceStatusIcon status="done" />
                ) : (
                  <AgentActionOrb />
                )}
              </span>
              <span className="thread-tool-chain-subagent-label">{subagent.description}</span>
            </div>
          ))}
        </div>
      )}
      {open && (
        <div className="thread-tool-chain-items">
          {row.items
            .filter((item) => !isToolRequestPlaceholder(item))
            .map((item) => (
              <ToolChainItem
                item={item}
                selected={selectedId === item.id}
                onSelect={onSelect}
                key={item.id}
              />
            ))}
        </div>
      )}
    </details>
  );
});
