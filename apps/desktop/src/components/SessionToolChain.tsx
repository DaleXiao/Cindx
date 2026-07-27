import {
  Activity,
  ChevronRight,
  FileText,
  ShieldCheck,
  TerminalSquare
} from "lucide-react";
import { memo, useState } from "react";
import type { TimelineEntry } from "../tauri";
import { TraceStatusIcon } from "./TraceStatusIcon";
import {
  isToolRequestPlaceholder,
  type SessionThreadSelection,
  type ThreadRow
} from "./sessionThreadProjection";

export function EventIcon({ event }: { event: TimelineEntry }) {
  if (event.kind === "tool") return <TerminalSquare aria-hidden="true" />;
  if (event.kind === "permission") return <ShieldCheck aria-hidden="true" />;
  if (event.kind === "model") return <Activity aria-hidden="true" />;
  return <FileText aria-hidden="true" />;
}

export function toolMessageSummary(content: string) {
  const tool = content.match(/^tool=(.+)$/m)?.[1]?.trim();
  const status = content.match(/^status=(.+)$/m)?.[1]?.trim();
  return {
    label: tool || "Tool output",
    status: status || "done"
  };
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
      <TerminalSquare aria-hidden="true" />
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
  onSelect
}: {
  row: Extract<ThreadRow, { type: "tool-chain" }>;
  selectedId: string | null;
  onSelect: (selection: SessionThreadSelection) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = row.items.some((item) => item.id === selectedId);

  return (
    <details
      className={`thread-tool-chain ${selected ? "selected" : ""}`}
      data-minimap-id={row.id}
      data-minimap-index={row.itemIndex}
      data-minimap-kind="tool-chain"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <strong>Agent actions</strong>
        <ChevronRight className="thread-tool-chain-chevron" aria-hidden="true" />
      </summary>
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
