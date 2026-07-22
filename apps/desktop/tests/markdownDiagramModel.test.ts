import assert from "node:assert/strict";
import test from "node:test";
import { markdownDiagramForCode } from "../src/components/markdownDiagramModel.ts";

test("mermaid fences become renderable diagrams", () => {
  assert.deepEqual(markdownDiagramForCode("mermaid", "graph TD\n  A --> B\n"), {
    kind: "mermaid",
    source: "graph TD\n  A --> B"
  });
});

test("mindmap fences preserve Markdown hierarchy for Markmap", () => {
  assert.deepEqual(markdownDiagramForCode("mindmap", "# Cindx\n## Memory\n- Retrieval"), {
    kind: "mindmap",
    source: "# Cindx\n## Memory\n- Retrieval"
  });
});

test("legacy Mermaid mind maps are converted to Markmap Markdown", () => {
  assert.deepEqual(
    markdownDiagramForCode(
      "mindmap",
      "mindmap\n  root((Cindx))\n    Memory\n      Retrieval\n    Tools"
    ),
    {
      kind: "mindmap",
      source: "# Cindx\n- Memory\n  - Retrieval\n- Tools"
    }
  );
});

test("ordinary and empty code fences stay as code", () => {
  assert.equal(markdownDiagramForCode("rust", "fn main() {}"), null);
  assert.equal(markdownDiagramForCode("mermaid", "  \n"), null);
  assert.equal(markdownDiagramForCode("mindmap", "mindmap\n  \n"), null);
});
