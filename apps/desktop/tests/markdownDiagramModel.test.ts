import assert from "node:assert/strict";
import test from "node:test";
import { markdownDiagramForCode } from "../src/components/markdownDiagramModel.ts";

test("mermaid fences become renderable diagrams", () => {
  assert.deepEqual(markdownDiagramForCode("mermaid", "graph TD\n  A --> B\n"), {
    kind: "mermaid",
    source: "graph TD\n  A --> B"
  });
});

test("mindmap fences receive the Mermaid mindmap declaration", () => {
  assert.deepEqual(markdownDiagramForCode("mindmap", "root((Cindx))\n  Memory"), {
    kind: "mindmap",
    source: "mindmap\n  root((Cindx))\n    Memory"
  });
});

test("explicit mindmap declarations are not duplicated", () => {
  assert.deepEqual(markdownDiagramForCode("mindmap", "mindmap\n  root((Cindx))"), {
    kind: "mindmap",
    source: "mindmap\n  root((Cindx))"
  });
});

test("ordinary and empty code fences stay as code", () => {
  assert.equal(markdownDiagramForCode("rust", "fn main() {}"), null);
  assert.equal(markdownDiagramForCode("mermaid", "  \n"), null);
});
