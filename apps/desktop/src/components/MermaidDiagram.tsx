import { memo, useEffect, useState } from "react";
import type { MarkdownDiagram } from "./markdownDiagramModel";

type MermaidDiagramProps = MarkdownDiagram;

type RenderState =
  | { status: "loading" }
  | { status: "ready"; svg: string }
  | { status: "error"; message: string };

const MAX_DIAGRAM_SOURCE_LENGTH = 50_000;
let renderSequence = 0;
let renderQueue: Promise<void> = Promise.resolve();

function enqueueRender<T>(task: () => Promise<T>): Promise<T> {
  const result = renderQueue.then(task, task);
  renderQueue = result.then(
    () => undefined,
    () => undefined
  );
  return result;
}

async function renderDiagram(source: string) {
  if (source.length > MAX_DIAGRAM_SOURCE_LENGTH) {
    throw new Error("Diagram source is too large to render safely");
  }
  return enqueueRender(async () => {
    const { default: mermaid } = await import("mermaid");
    const dark = document.documentElement.dataset.theme === "dark";
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: dark ? "dark" : "neutral",
      flowchart: { htmlLabels: false },
      maxTextSize: MAX_DIAGRAM_SOURCE_LENGTH
    });
    renderSequence += 1;
    const result = await mermaid.render(`cindx-mermaid-${renderSequence}`, source);
    return result.svg;
  });
}

function renderErrorMessage(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message.split("\n", 1)[0] || "Invalid diagram source";
}

export const MermaidDiagram = memo(function MermaidDiagram({
  kind,
  source
}: MermaidDiagramProps) {
  const [renderState, setRenderState] = useState<RenderState>({ status: "loading" });

  useEffect(() => {
    let active = true;
    setRenderState({ status: "loading" });
    void renderDiagram(source).then(
      (svg) => {
        if (active) setRenderState({ status: "ready", svg });
      },
      (error) => {
        if (active) {
          setRenderState({ status: "error", message: renderErrorMessage(error) });
        }
      }
    );
    return () => {
      active = false;
    };
  }, [source]);

  if (renderState.status === "loading") {
    return (
      <div className="thread-diagram-loading" role="status">
        Rendering {kind === "mindmap" ? "mind map" : "diagram"}...
      </div>
    );
  }
  if (renderState.status === "error") {
    return (
      <div className="thread-diagram-error">
        <p>Could not render diagram: {renderState.message}</p>
        <pre>
          <code>{source}</code>
        </pre>
      </div>
    );
  }
  return (
    <div
      className="thread-mermaid-diagram"
      role="img"
      aria-label={kind === "mindmap" ? "Mind map" : "Mermaid diagram"}
      dangerouslySetInnerHTML={{ __html: renderState.svg }}
    />
  );
});
