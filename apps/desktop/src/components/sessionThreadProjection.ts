import type {
  AgentOutputArtifactView,
  ChatMessageView,
  TimelineEntry
} from "../tauri";

export type SessionThreadSelection =
  | {
      id: string;
      type: "message";
      message: ChatMessageView;
    }
  | {
      id: string;
      type: "event";
      event: TimelineEntry;
    };

export type MinimapMarker = {
  id: string;
  kind: string;
  label: string;
  preview: string;
  targetIndex: number;
};

export type ThreadRow =
  | {
      type: "item";
      item: SessionThreadSelection;
      itemIndex: number;
    }
  | {
      type: "tool-chain";
      id: string;
      items: SessionThreadSelection[];
      itemIndex: number;
    };

type AssistantItem = Extract<SessionThreadSelection, { type: "message" }>;

export type SessionThreadProjection = {
  sourceMessages: ChatMessageView[];
  sourceTimeline: TimelineEntry[];
  visibleTimelineItemCount: number;
  items: SessionThreadSelection[];
  rows: ThreadRow[];
  rowIndexByItemId: Map<string, number>;
  rawMinimapMarkers: MinimapMarker[];
  minimapMarkers: MinimapMarker[];
  assistants: AssistantItem[];
  lastAssistantByRunId: Map<string, AssistantItem>;
  latestVisibleUserTimestamp: number;
};

export function threadMessageId(message: ChatMessageView, index: number) {
  return `message-${message.sequence ?? `${message.role}-${index}`}`;
}

export function isToolRequestPlaceholder(item: SessionThreadSelection) {
  const content = item.type === "message" ? item.message.content.trim().toLowerCase() : "";
  return (
    item.type === "message" &&
    item.message.role === "assistant" &&
    (!content || content === "tool request")
  );
}

function isActivityCandidate(item: SessionThreadSelection) {
  return (
    item.type === "event" ||
    (item.type === "message" && item.message.role === "tool") ||
    isToolRequestPlaceholder(item)
  );
}

export function threadRowKey(row: ThreadRow) {
  return row.type === "tool-chain" ? row.id : row.item.id;
}

function visibleTimelineItems(timeline: TimelineEntry[], indexOffset = 0) {
  const items: Extract<SessionThreadSelection, { type: "event" }>[] = [];
  timeline.forEach((event) => {
    if (
      event.kind === "message" ||
      (event.kind === "model" &&
        (event.label === "Model started" || event.label === "Model finished"))
    ) {
      return;
    }
    items.push({
      id: `event-${event.sequence ?? `${event.timestampMs}-${indexOffset + items.length}`}`,
      type: "event",
      event
    });
  });
  return items;
}

function projectItems(
  messages: ChatMessageView[],
  timeline: TimelineEntry[],
  messageIndexOffset = 0,
  eventIndexOffset = 0
) {
  const messageItems = messages.map((message, index) => ({
    id: threadMessageId(message, messageIndexOffset + index),
    type: "message" as const,
    message
  }));
  const eventItems = visibleTimelineItems(timeline, eventIndexOffset);
  const items: SessionThreadSelection[] = [];
  let messageIndex = 0;
  let eventIndex = 0;
  while (messageIndex < messageItems.length || eventIndex < eventItems.length) {
    const message = messageItems[messageIndex];
    const event = eventItems[eventIndex];
    if (message && (!event || message.message.timestampMs <= event.event.timestampMs)) {
      items.push(message);
      messageIndex += 1;
    } else {
      items.push(event);
      eventIndex += 1;
    }
  }
  return { items, visibleTimelineItemCount: eventItems.length };
}

function collectMessageMetadata(items: SessionThreadSelection[], targetIndexOffset = 0) {
  const rawMinimapMarkers: MinimapMarker[] = [];
  const assistants: AssistantItem[] = [];
  const lastAssistantByRunId = new Map<string, AssistantItem>();
  let latestVisibleUserTimestamp = 0;
  items.forEach((item, index) => {
    if (item.type !== "message") return;
    if (item.message.role === "assistant") {
      assistants.push(item);
      if (item.message.runId) lastAssistantByRunId.set(item.message.runId, item);
    } else if (item.message.role === "user") {
      latestVisibleUserTimestamp = Math.max(
        latestVisibleUserTimestamp,
        item.message.timestampMs
      );
    }

    const role = item.message.role;
    const preview = item.message.content.trim();
    if (
      (role === "user" || role === "assistant" || role === "reviewer") &&
      preview &&
      preview.toLowerCase() !== "tool request"
    ) {
      rawMinimapMarkers.push({
        id: item.id,
        kind: role,
        label:
          role === "user"
            ? "You"
            : role === "assistant"
              ? "Cindx"
              : `${role.charAt(0).toUpperCase()}${role.slice(1)}`,
        preview,
        targetIndex: targetIndexOffset + index
      });
    }
  });
  return {
    rawMinimapMarkers,
    assistants,
    lastAssistantByRunId,
    latestVisibleUserTimestamp
  };
}

function buildRows(items: SessionThreadSelection[]) {
  const rows: ThreadRow[] = [];
  const rowIndexByItemId = new Map<string, number>();
  appendRows(rows, rowIndexByItemId, items, 0);
  return { rows, rowIndexByItemId };
}

function appendRows(
  rows: ThreadRow[],
  rowIndexByItemId: Map<string, number>,
  items: SessionThreadSelection[],
  itemIndexOffset: number
) {
  let index = 0;
  const lastRowIndex = rows.length - 1;
  const lastRow = rows[lastRowIndex];
  if (lastRow?.type === "tool-chain" && isActivityCandidate(items[0])) {
    let end = 1;
    while (end < items.length && isActivityCandidate(items[end])) end += 1;
    const appended = items.slice(0, end);
    rows[lastRowIndex] = { ...lastRow, items: [...lastRow.items, ...appended] };
    appended.forEach((item) => rowIndexByItemId.set(item.id, lastRowIndex));
    index = end;
  }

  while (index < items.length) {
    const rowIndex = rows.length;
    const globalItemIndex = itemIndexOffset + index;
    if (!isActivityCandidate(items[index])) {
      const item = items[index];
      rows.push({ type: "item", item, itemIndex: globalItemIndex });
      rowIndexByItemId.set(item.id, rowIndex);
      index += 1;
      continue;
    }

    let end = index + 1;
    while (end < items.length && isActivityCandidate(items[end])) end += 1;
    const candidates = items.slice(index, end);
    rows.push({
      type: "tool-chain",
      id: `tool-chain-${candidates[0].id}`,
      items: candidates,
      itemIndex: globalItemIndex
    });
    candidates.forEach((candidate) => rowIndexByItemId.set(candidate.id, rowIndex));
    index = end;
  }
}

function downsampleMarkers(markers: MinimapMarker[], limit: number) {
  if (markers.length <= limit) return markers;
  return Array.from({ length: limit }, (_, index) => {
    const sourceIndex = Math.round((index * (markers.length - 1)) / (limit - 1));
    return markers[sourceIndex];
  });
}

export function buildSessionThreadProjection(
  messages: ChatMessageView[],
  timeline: TimelineEntry[],
  minimapMarkerLimit: number
): SessionThreadProjection {
  const projected = projectItems(messages, timeline);
  const metadata = collectMessageMetadata(projected.items);
  const rowProjection = buildRows(projected.items);

  return {
    sourceMessages: messages,
    sourceTimeline: timeline,
    visibleTimelineItemCount: projected.visibleTimelineItemCount,
    items: projected.items,
    rows: rowProjection.rows,
    rowIndexByItemId: rowProjection.rowIndexByItemId,
    rawMinimapMarkers: metadata.rawMinimapMarkers,
    minimapMarkers: downsampleMarkers(metadata.rawMinimapMarkers, minimapMarkerLimit),
    assistants: metadata.assistants,
    lastAssistantByRunId: metadata.lastAssistantByRunId,
    latestVisibleUserTimestamp: metadata.latestVisibleUserTimestamp
  };
}

function hasIdentityPrefix<Item>(previous: Item[], next: Item[]) {
  if (previous === next) return true;
  if (next.length < previous.length) return false;
  for (let index = 0; index < previous.length; index += 1) {
    if (previous[index] !== next[index]) return false;
  }
  return true;
}

function itemTimestamp(item: SessionThreadSelection) {
  return item.type === "message" ? item.message.timestampMs : item.event.timestampMs;
}

export function updateSessionThreadProjection(
  previous: SessionThreadProjection | null,
  messages: ChatMessageView[],
  timeline: TimelineEntry[],
  minimapMarkerLimit: number
) {
  if (!previous) {
    return buildSessionThreadProjection(messages, timeline, minimapMarkerLimit);
  }
  if (previous.sourceMessages === messages && previous.sourceTimeline === timeline) {
    return previous;
  }
  if (
    !hasIdentityPrefix(previous.sourceMessages, messages) ||
    !hasIdentityPrefix(previous.sourceTimeline, timeline)
  ) {
    return buildSessionThreadProjection(messages, timeline, minimapMarkerLimit);
  }

  const appended = projectItems(
    messages.slice(previous.sourceMessages.length),
    timeline.slice(previous.sourceTimeline.length),
    previous.sourceMessages.length,
    previous.visibleTimelineItemCount
  );
  const previousLastItem = previous.items[previous.items.length - 1];
  if (
    appended.items.length > 0 &&
    previousLastItem &&
    itemTimestamp(appended.items[0]) <= itemTimestamp(previousLastItem)
  ) {
    return buildSessionThreadProjection(messages, timeline, minimapMarkerLimit);
  }
  if (appended.items.length === 0) {
    return {
      ...previous,
      sourceMessages: messages,
      sourceTimeline: timeline,
      visibleTimelineItemCount:
        previous.visibleTimelineItemCount + appended.visibleTimelineItemCount
    };
  }

  const itemIndexOffset = previous.items.length;
  const metadata = collectMessageMetadata(appended.items, itemIndexOffset);
  const rows = previous.rows.slice();
  const rowIndexByItemId = new Map(previous.rowIndexByItemId);
  appendRows(rows, rowIndexByItemId, appended.items, itemIndexOffset);
  const lastAssistantByRunId = new Map(previous.lastAssistantByRunId);
  metadata.lastAssistantByRunId.forEach((item, runId) => {
    lastAssistantByRunId.set(runId, item);
  });
  const rawMinimapMarkers = [
    ...previous.rawMinimapMarkers,
    ...metadata.rawMinimapMarkers
  ];

  return {
    sourceMessages: messages,
    sourceTimeline: timeline,
    visibleTimelineItemCount:
      previous.visibleTimelineItemCount + appended.visibleTimelineItemCount,
    items: [...previous.items, ...appended.items],
    rows,
    rowIndexByItemId,
    rawMinimapMarkers,
    minimapMarkers: downsampleMarkers(rawMinimapMarkers, minimapMarkerLimit),
    assistants: [...previous.assistants, ...metadata.assistants],
    lastAssistantByRunId,
    latestVisibleUserTimestamp: Math.max(
      previous.latestVisibleUserTimestamp,
      metadata.latestVisibleUserTimestamp
    )
  };
}

export function sessionMinimapMarkers(
  projection: SessionThreadProjection,
  streamAnswer: string,
  limit: number
) {
  const preview = streamAnswer.trim();
  if (!preview) return projection.minimapMarkers;
  return downsampleMarkers(
    [
      ...projection.rawMinimapMarkers,
      {
        id: "streaming-answer",
        kind: "streaming",
        label: "Cindx",
        preview,
        targetIndex: projection.items.length
      }
    ],
    limit
  );
}

function firstAssistantAtOrAfter(assistants: AssistantItem[], timestampMs: number) {
  let low = 0;
  let high = assistants.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (assistants[middle].message.timestampMs < timestampMs) low = middle + 1;
    else high = middle;
  }
  return assistants[low];
}

export function associateOutputArtifacts(
  projection: SessionThreadProjection,
  outputArtifacts: AgentOutputArtifactView[]
) {
  const artifactsByMessageId = new Map<string, AgentOutputArtifactView[]>();
  const trailingArtifacts: AgentOutputArtifactView[] = [];
  [...outputArtifacts]
    .sort((left, right) => left.timestampMs - right.timestampMs)
    .forEach((artifact) => {
      const target =
        (artifact.runId
          ? projection.lastAssistantByRunId.get(artifact.runId)
          : undefined) ??
        firstAssistantAtOrAfter(projection.assistants, artifact.timestampMs);
      if (target) {
        const current = artifactsByMessageId.get(target.id) ?? [];
        current.push(artifact);
        artifactsByMessageId.set(target.id, current);
      } else if (artifact.timestampMs >= projection.latestVisibleUserTimestamp) {
        trailingArtifacts.push(artifact);
      }
    });

  return { artifactsByMessageId, trailingArtifacts };
}
