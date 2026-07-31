export type ProjectMemoryItemState =
  | "active"
  | "disabled"
  | "quarantined"
  | "superseded";

export type ProjectMemoryAction =
  | "delete"
  | "disable"
  | "enable"
  | "pin"
  | "unpin"
  | "promote";

export type ProjectMemoryItem = {
  id: string;
  kind: "requirement" | "outcome" | "evidence";
  trust: "user_stated" | "tool_verified" | "assistant_reported" | "legacy_unverified";
  content: string;
  state: ProjectMemoryItemState;
  pinned: boolean;
  itemRevision: number;
  contentSha256: string;
  importance: number;
  sourceSessionIds: string[];
  createdAtMs: number;
  updatedAtMs: number;
  recallCount: number;
  observedUseCount: number;
  supersededBy: string | null;
  quarantineReason?: string | null;
};

export type ProjectMemoryState = {
  projectId: string;
  revision: number;
  items: ProjectMemoryItem[];
  activeCount: number;
  disabledCount: number;
  quarantinedCount: number;
  pinnedCount: number;
};

export type UpdateProjectMemoryInput = {
  projectId: string;
  memoryId: string;
  action: ProjectMemoryAction;
  expectedItemRevision: number;
  expectedContentSha256: string;
  confirmedContent?: string;
};

export type ProjectMemorySections = {
  active: ProjectMemoryItem[];
  disabled: ProjectMemoryItem[];
  quarantined: ProjectMemoryItem[];
};

export type ProjectMemoryStats = {
  records: number;
  requirements: number;
  evidence: number;
  recalls: number;
  observedUses: number;
};

export function partitionProjectMemories(items: ProjectMemoryItem[]): ProjectMemorySections {
  return {
    active: items.filter((item) => item.state === "active"),
    disabled: items.filter(
      (item) => item.state === "disabled" || item.state === "superseded"
    ),
    quarantined: items.filter((item) => item.state === "quarantined")
  };
}

export function summarizeProjectMemories(items: ProjectMemoryItem[]): ProjectMemoryStats {
  return items.reduce<ProjectMemoryStats>(
    (stats, item) => ({
      records: stats.records + 1,
      requirements: stats.requirements + Number(item.kind === "requirement"),
      evidence: stats.evidence + Number(item.kind === "evidence"),
      recalls: stats.recalls + item.recallCount,
      observedUses: stats.observedUses + item.observedUseCount
    }),
    { records: 0, requirements: 0, evidence: 0, recalls: 0, observedUses: 0 }
  );
}

export function projectMemoryActionAllowed(
  item: ProjectMemoryItem,
  action: ProjectMemoryAction
): boolean {
  if (action === "delete") return true;
  if (item.state === "superseded") return false;
  if (item.state === "quarantined") return action === "promote";
  if (item.state === "disabled") return action === "enable";
  if (action === "disable") return true;
  if (action === "pin") return !item.pinned;
  if (action === "unpin") return item.pinned;
  return false;
}

function previewContentFingerprint(content: string) {
  let hash = 2166136261;
  for (let index = 0; index < content.length; index += 1) {
    hash ^= content.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0).toString(16).padStart(8, "0").repeat(8);
}

function withProjectMemoryCounts(
  state: Pick<ProjectMemoryState, "projectId" | "revision" | "items">
): ProjectMemoryState {
  const sections = partitionProjectMemories(state.items);
  return {
    ...state,
    activeCount: sections.active.length,
    disabledCount: sections.disabled.length,
    quarantinedCount: sections.quarantined.length,
    pinnedCount: sections.active.filter((item) => item.pinned).length
  };
}

export function createBrowserProjectMemoryState(projectId: string): ProjectMemoryState {
  if (projectId !== "project-cindx") {
    return withProjectMemoryCounts({ projectId, revision: 0, items: [] });
  }
  const now = Date.now();
  return withProjectMemoryCounts({
    projectId,
    revision: 3,
    items: [
      {
        id: "memory-preview-requirement",
        kind: "requirement",
        trust: "user_stated",
        content: "Always preserve existing capabilities and verified historical fixes.",
        state: "active",
        pinned: true,
        itemRevision: 3,
        contentSha256: "a".repeat(64),
        importance: 96,
        sourceSessionIds: ["preview-session"],
        createdAtMs: now - 86_400_000,
        updatedAtMs: now - 3_600_000,
        recallCount: 4,
        observedUseCount: 3,
        supersededBy: null
      },
      {
        id: "memory-preview-disabled",
        kind: "evidence",
        trust: "tool_verified",
        content: "The workspace uses a Rust and Tauri desktop runtime.",
        state: "disabled",
        pinned: false,
        itemRevision: 2,
        contentSha256: "b".repeat(64),
        importance: 72,
        sourceSessionIds: ["preview-session"],
        createdAtMs: now - 172_800_000,
        updatedAtMs: now - 7_200_000,
        recallCount: 2,
        observedUseCount: 1,
        supersededBy: null
      },
      {
        id: "memory-preview-quarantined",
        kind: "requirement",
        trust: "legacy_unverified",
        content: "Prefer concise release summaries.",
        state: "quarantined",
        pinned: false,
        itemRevision: 1,
        contentSha256: "c".repeat(64),
        importance: 80,
        sourceSessionIds: ["preview-session"],
        createdAtMs: now - 10_800_000,
        updatedAtMs: now - 10_800_000,
        recallCount: 0,
        observedUseCount: 0,
        supersededBy: null,
        quarantineReason: "Model wording is not backed by a verbatim durable user requirement."
      }
    ]
  });
}

export function applyBrowserProjectMemoryUpdate(
  state: ProjectMemoryState,
  input: UpdateProjectMemoryInput,
  nowMs = Date.now()
): ProjectMemoryState {
  if (state.projectId !== input.projectId) throw new Error("project memory changed");
  const item = state.items.find((candidate) => candidate.id === input.memoryId);
  if (!item) throw new Error("project memory no longer exists");
  if (
    item.itemRevision !== input.expectedItemRevision ||
    item.contentSha256 !== input.expectedContentSha256
  ) {
    throw new Error("project memory changed; refresh and try again");
  }
  if (!projectMemoryActionAllowed(item, input.action)) {
    throw new Error(`memory action ${input.action} is not allowed for ${item.state} memory`);
  }
  if (input.action === "delete") {
    return withProjectMemoryCounts({
      projectId: state.projectId,
      revision: state.revision + 1,
      items: state.items.filter((candidate) => candidate.id !== item.id)
    });
  }
  const confirmedContent = input.confirmedContent?.trim();
  if (input.action === "promote" && !confirmedContent) {
    throw new Error("review the memory text before saving it as a project requirement");
  }
  const next = state.items.map((candidate) => {
    if (candidate.id !== item.id) return candidate;
    const content = input.action === "promote" ? confirmedContent! : candidate.content;
    return {
      ...candidate,
      id:
        input.action === "promote"
          ? `memory-confirmed-${previewContentFingerprint(content).slice(0, 16)}`
          : candidate.id,
      content,
      contentSha256:
        input.action === "promote"
          ? previewContentFingerprint(content)
          : candidate.contentSha256,
      state:
        input.action === "disable"
          ? "disabled"
          : input.action === "enable" || input.action === "promote"
            ? "active"
            : candidate.state,
      pinned:
        input.action === "pin"
          ? true
          : input.action === "unpin"
            ? false
            : candidate.pinned,
      trust: input.action === "promote" ? "user_stated" : candidate.trust,
      kind: input.action === "promote" ? "requirement" : candidate.kind,
      quarantineReason: input.action === "promote" ? null : candidate.quarantineReason,
      itemRevision: candidate.itemRevision + 1,
      updatedAtMs: nowMs
    } satisfies ProjectMemoryItem;
  });
  return withProjectMemoryCounts({
    projectId: state.projectId,
    revision: state.revision + 1,
    items: next
  });
}

const browserProjectMemoryStates = new Map<string, ProjectMemoryState>();

export function getBrowserProjectMemoryState(projectId: string) {
  const current = browserProjectMemoryStates.get(projectId);
  if (current) return current;
  const created = createBrowserProjectMemoryState(projectId);
  browserProjectMemoryStates.set(projectId, created);
  return created;
}

export function updateBrowserProjectMemory(input: UpdateProjectMemoryInput) {
  const next = applyBrowserProjectMemoryUpdate(
    getBrowserProjectMemoryState(input.projectId),
    input
  );
  browserProjectMemoryStates.set(input.projectId, next);
  return next;
}
