import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("knowledge graph refresh preserves a loaded preview while cold startup stays lightweight", () => {
  const controller = readFileSync(
    new URL("../src/controllers/useKnowledgeToolingController.ts", import.meta.url),
    "utf8"
  );
  const settings = readFileSync(
    new URL("../src/components/SettingsPage.tsx", import.meta.url),
    "utf8"
  );

  assert.match(controller, /void getPhase7State\(\)/);
  assert.match(
    controller,
    /knowledgeGraphOpen \? ensureWorkspaceKnowledge\(\) : getPhase7State\(\)/
  );
  assert.match(controller, /\}, \[knowledgeGraphOpen\]\);/);
  assert.match(settings, /ragBusy && \(phase7\?\.graph\.nodes\.length \?\? 0\) === 0/);
  assert.doesNotMatch(
    settings,
    /ragBusy && \(phase7\?\.graph\.totalNodes \?\? 0\) === 0/
  );
});
