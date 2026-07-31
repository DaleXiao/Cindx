import assert from "node:assert/strict";
import test from "node:test";
import {
  applyBrowserProjectMemoryUpdate,
  createBrowserProjectMemoryState,
  partitionProjectMemories,
  projectMemoryActionAllowed,
  summarizeProjectMemories,
  type ProjectMemoryItem
} from "../src/memoryManagementModel.ts";

function item(
  overrides: Partial<ProjectMemoryItem> & Pick<ProjectMemoryItem, "id" | "state">
): ProjectMemoryItem {
  return {
    id: overrides.id,
    kind: "requirement",
    trust: "user_stated",
    content: `Memory ${overrides.id}`,
    state: overrides.state,
    pinned: false,
    itemRevision: 1,
    contentSha256: overrides.id.padEnd(64, "0").slice(0, 64),
    importance: 80,
    sourceSessionIds: ["session-1"],
    createdAtMs: 1,
    updatedAtMs: 1,
    recallCount: 0,
    observedUseCount: 0,
    supersededBy: null,
    ...overrides
  };
}

test("memory sections keep superseded records inactive and quarantine separate", () => {
  const sections = partitionProjectMemories([
    item({ id: "active", state: "active" }),
    item({ id: "disabled", state: "disabled" }),
    item({ id: "superseded", state: "superseded", supersededBy: "active" }),
    item({ id: "review", state: "quarantined" })
  ]);
  assert.deepEqual(sections.active.map(({ id }) => id), ["active"]);
  assert.deepEqual(sections.disabled.map(({ id }) => id), ["disabled", "superseded"]);
  assert.deepEqual(sections.quarantined.map(({ id }) => id), ["review"]);
});

test("available memory actions follow the server state contract", () => {
  const active = item({ id: "active", state: "active" });
  const pinned = item({ id: "pinned", state: "active", pinned: true });
  const disabled = item({ id: "disabled", state: "disabled" });
  const superseded = item({ id: "superseded", state: "superseded" });
  const quarantined = item({ id: "review", state: "quarantined" });
  assert.equal(projectMemoryActionAllowed(active, "disable"), true);
  assert.equal(projectMemoryActionAllowed(active, "pin"), true);
  assert.equal(projectMemoryActionAllowed(active, "promote"), false);
  assert.equal(projectMemoryActionAllowed(pinned, "unpin"), true);
  assert.equal(projectMemoryActionAllowed(pinned, "pin"), false);
  assert.equal(projectMemoryActionAllowed(disabled, "enable"), true);
  assert.equal(projectMemoryActionAllowed(disabled, "pin"), false);
  assert.equal(projectMemoryActionAllowed(superseded, "enable"), false);
  assert.equal(projectMemoryActionAllowed(superseded, "delete"), true);
  assert.equal(projectMemoryActionAllowed(quarantined, "promote"), true);
  assert.equal(projectMemoryActionAllowed(quarantined, "enable"), false);
});

test("memory summaries preserve the existing Settings statistics", () => {
  const stats = summarizeProjectMemories([
    item({ id: "requirement", state: "active", recallCount: 3, observedUseCount: 2 }),
    item({
      id: "evidence",
      state: "disabled",
      kind: "evidence",
      trust: "tool_verified",
      recallCount: 4,
      observedUseCount: 1
    }),
    item({
      id: "outcome",
      state: "active",
      kind: "outcome",
      trust: "assistant_reported"
    })
  ]);
  assert.deepEqual(stats, {
    records: 3,
    requirements: 1,
    evidence: 1,
    recalls: 7,
    observedUses: 3
  });
});

test("pinning changes priority state without changing memory trust or content", () => {
  const before = createBrowserProjectMemoryState("project-cindx");
  const target = before.items.find(({ state }) => state === "active");
  assert.ok(target);
  const after = applyBrowserProjectMemoryUpdate(
    before,
    {
      projectId: before.projectId,
      memoryId: target.id,
      action: "unpin",
      expectedItemRevision: target.itemRevision,
      expectedContentSha256: target.contentSha256
    },
    99
  );
  const updated = after.items.find(({ id }) => id === target.id);
  assert.ok(updated);
  assert.equal(updated.pinned, false);
  assert.equal(updated.kind, target.kind);
  assert.equal(updated.trust, target.trust);
  assert.equal(updated.content, target.content);
  assert.equal(updated.itemRevision, target.itemRevision + 1);
  assert.equal(after.revision, before.revision + 1);
});

test("disabling hides recall priority without erasing the pinned preference", () => {
  const before = createBrowserProjectMemoryState("project-cindx");
  const target = before.items.find((candidate) => candidate.state === "active" && candidate.pinned);
  assert.ok(target);
  const disabled = applyBrowserProjectMemoryUpdate(before, {
    projectId: before.projectId,
    memoryId: target.id,
    action: "disable",
    expectedItemRevision: target.itemRevision,
    expectedContentSha256: target.contentSha256
  });
  const updated = disabled.items.find(({ id }) => id === target.id);
  assert.equal(updated?.state, "disabled");
  assert.equal(updated?.pinned, true);
  assert.equal(disabled.pinnedCount, 0);
});

test("quarantined memory requires explicit reviewed content before promotion", () => {
  const before = createBrowserProjectMemoryState("project-cindx");
  const target = before.items.find(({ state }) => state === "quarantined");
  assert.ok(target);
  assert.throws(
    () =>
      applyBrowserProjectMemoryUpdate(before, {
        projectId: before.projectId,
        memoryId: target.id,
        action: "promote",
        expectedItemRevision: target.itemRevision,
        expectedContentSha256: target.contentSha256
      }),
    /review the memory text/
  );
  const after = applyBrowserProjectMemoryUpdate(before, {
    projectId: before.projectId,
    memoryId: target.id,
    action: "promote",
    expectedItemRevision: target.itemRevision,
    expectedContentSha256: target.contentSha256,
    confirmedContent: "Always keep release summaries concise."
  });
  const updated = after.items.find(
    ({ content, state }) =>
      state === "active" && content === "Always keep release summaries concise."
  );
  assert.equal(updated?.state, "active");
  assert.equal(updated?.kind, "requirement");
  assert.equal(updated?.trust, "user_stated");
  assert.equal(updated?.content, "Always keep release summaries concise.");
  assert.equal(after.items.some(({ id }) => id === target.id), false);
});

test("stale revisions and content hashes cannot mutate or delete another memory version", () => {
  const before = createBrowserProjectMemoryState("project-cindx");
  const target = before.items[0];
  assert.throws(
    () =>
      applyBrowserProjectMemoryUpdate(before, {
        projectId: before.projectId,
        memoryId: target.id,
        action: "delete",
        expectedItemRevision: target.itemRevision - 1,
        expectedContentSha256: target.contentSha256
      }),
    /changed; refresh/
  );
  assert.throws(
    () =>
      applyBrowserProjectMemoryUpdate(before, {
        projectId: before.projectId,
        memoryId: target.id,
        action: "delete",
        expectedItemRevision: target.itemRevision,
        expectedContentSha256: "f".repeat(64)
      }),
    /changed; refresh/
  );
  const after = applyBrowserProjectMemoryUpdate(before, {
    projectId: before.projectId,
    memoryId: target.id,
    action: "delete",
    expectedItemRevision: target.itemRevision,
    expectedContentSha256: target.contentSha256
  });
  assert.equal(after.items.some(({ id }) => id === target.id), false);
});
