import type { TimelineEntry } from "../tauriTypes";

export type RunSubagentState = { description: string; done: boolean };
export type RunProgress = { label: string; detail: string; subagents: RunSubagentState[] };

export function activeRunProgress(timeline: TimelineEntry[], runStartedAtMs: number): RunProgress {
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
  const subagents: RunSubagentState[] = [];
  const subagentByDescription = new Map<string, RunSubagentState>();
  for (const event of timeline) {
    if (event.timestampMs < startedAtMs || event.label !== "Status") continue;
    const match = event.detail.match(/^Subagent (started|finished): (.+)$/i);
    if (!match) continue;
    const description = match[2].trim();
    let entry = subagentByDescription.get(description);
    if (!entry) {
      entry = { description, done: false };
      subagentByDescription.set(description, entry);
      subagents.push(entry);
    }
    entry.done = match[1].toLowerCase() === "finished";
  }
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
      detail: "Cindx is working",
      subagents
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
  } else if (latest.label === "Status" && /subagent started/i.test(latest.detail)) {
    const done = subagents.filter((subagent) => subagent.done).length;
    label =
      subagents.length > 1
        ? `Running subagents ${done}/${subagents.length}`
        : "Running subagent";
  } else if (latest.label === "Status" && /subagent finished/i.test(latest.detail)) {
    label = "Subagent done";
  }

  return { label, detail: latest.detail, subagents };
}
