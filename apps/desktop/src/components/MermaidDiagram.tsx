import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useResolvedTheme, type ResolvedTheme } from "./useResolvedTheme";

type MermaidDiagramProps = {
  source: string;
};

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

async function renderDiagram(source: string, theme: ResolvedTheme) {
  if (source.length > MAX_DIAGRAM_SOURCE_LENGTH) {
    throw new Error("Diagram source is too large to render safely");
  }
  return enqueueRender(async () => {
    const { default: mermaid } = await import("mermaid");
    const dark = theme === "dark";
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: dark ? "dark" : "neutral",
      themeVariables: dark
        ? {
            background: "#171717",
            primaryColor: "#292929",
            primaryTextColor: "#f3f3f3",
            primaryBorderColor: "#858585",
            lineColor: "#c2c2c2",
            secondaryColor: "#242424",
            tertiaryColor: "#303030",
            textColor: "#f3f3f3",
            edgeLabelBackground: "#171717",
            clusterBkg: "#202020",
            clusterBorder: "#6f6f6f",
            noteBkgColor: "#303030",
            noteTextColor: "#f3f3f3",
            noteBorderColor: "#858585"
          }
        : undefined,
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

function contrastingTextColor(fill: string) {
  const channels = fill.match(/[\d.]+/g)?.map(Number);
  if (!channels || channels.length < 3 || channels.some(Number.isNaN)) return null;
  const [red, green, blue, alpha = 1] = channels;
  if (alpha === 0) return null;
  const linearize = (channel: number) => {
    const normalized = channel / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  };
  const luminance =
    0.2126 * linearize(red) +
    0.7152 * linearize(green) +
    0.0722 * linearize(blue);
  return luminance > 0.179 ? "#171717" : "#f3f3f3";
}

function applyDarkDiagramLabelContrast(container: HTMLDivElement) {
  container.querySelectorAll<SVGGElement>("svg g.node").forEach((node) => {
    const shape = node.querySelector<SVGGraphicsElement>(
      ":scope > rect, :scope > circle, :scope > ellipse, :scope > polygon, :scope > path"
    );
    if (!shape) return;
    const labelColor = contrastingTextColor(getComputedStyle(shape).fill);
    if (!labelColor) return;
    node.querySelectorAll<SVGElement>("text, tspan").forEach((label) => {
      label.style.setProperty("fill", labelColor, "important");
    });
    node.querySelectorAll<HTMLElement>("foreignObject *").forEach((label) => {
      label.style.setProperty("color", labelColor, "important");
    });
  });
}

export const MermaidDiagram = memo(function MermaidDiagram({
  source
}: MermaidDiagramProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [renderState, setRenderState] = useState<RenderState>({ status: "loading" });
  const theme = useResolvedTheme();

  useEffect(() => {
    let active = true;
    setRenderState({ status: "loading" });
    void renderDiagram(source, theme).then(
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
  }, [source, theme]);

  useLayoutEffect(() => {
    if (theme !== "dark" || renderState.status !== "ready" || !containerRef.current) return;
    applyDarkDiagramLabelContrast(containerRef.current);
  }, [renderState, theme]);

  if (renderState.status === "loading") {
    return (
      <div className="thread-diagram-loading" role="status">
        Rendering diagram...
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
      ref={containerRef}
      className="thread-mermaid-diagram"
      data-theme={theme}
      role="img"
      aria-label="Mermaid diagram"
      dangerouslySetInnerHTML={{ __html: renderState.svg }}
    />
  );
});
