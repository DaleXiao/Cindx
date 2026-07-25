import { memo, useEffect, useRef, useState } from "react";
import { useResolvedTheme } from "./useResolvedTheme";

type MarkmapDiagramProps = {
  source: string;
  scale?: number;
};

type RenderState =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; message: string };

type MarkmapRuntime = {
  Transformer: typeof import("markmap-lib").Transformer;
  Markmap: typeof import("markmap-view").Markmap;
};

const MAX_MINDMAP_SOURCE_LENGTH = 50_000;
let runtimePromise: Promise<MarkmapRuntime> | null = null;

function loadRuntime() {
  runtimePromise ??= Promise.all([import("markmap-lib"), import("markmap-view")]).then(
    ([markmapLib, markmapView]) => ({
      Transformer: markmapLib.Transformer,
      Markmap: markmapView.Markmap
    })
  );
  return runtimePromise;
}

function renderErrorMessage(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message.split("\n", 1)[0] || "Invalid mind map source";
}

export const MarkmapDiagram = memo(function MarkmapDiagram({
  source,
  scale = 1
}: MarkmapDiagramProps) {
  const svgRef = useRef<SVGSVGElement>(null);
  const markmapRef = useRef<InstanceType<MarkmapRuntime["Markmap"]> | null>(null);
  const appliedScaleRef = useRef(1);
  const requestedScaleRef = useRef(scale);
  const [renderState, setRenderState] = useState<RenderState>({ status: "loading" });
  const theme = useResolvedTheme();
  requestedScaleRef.current = scale;

  useEffect(() => {
    const markmap = markmapRef.current;
    if (!markmap || appliedScaleRef.current === scale) return;
    const ratio = scale / appliedScaleRef.current;
    appliedScaleRef.current = scale;
    void markmap.rescale(ratio);
  }, [scale]);

  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    let active = true;
    let markmap: InstanceType<MarkmapRuntime["Markmap"]> | null = null;
    svg.replaceChildren();
    setRenderState({ status: "loading" });

    void (async () => {
      try {
        if (source.length > MAX_MINDMAP_SOURCE_LENGTH) {
          throw new Error("Mind map source is too large to render safely");
        }
        const { Transformer, Markmap } = await loadRuntime();
        if (!active) return;
        const transformer = new Transformer();
        transformer.md.set({ html: false });
        const { root } = transformer.transform(source);
        const dark = theme === "dark";
        const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
        markmap = new Markmap(svg, {
          autoFit: true,
          color: () => (dark ? "#8ab4f8" : "#2563eb"),
          duration: reduceMotion ? 0 : 220,
          embedGlobalCSS: true,
          fitRatio: 0.92,
          lineWidth: (node) => (node.state.depth === 0 ? 1.8 : 1.25),
          maxInitialScale: 1.15,
          maxWidth: 280,
          nodeMinHeight: 18,
          paddingX: 10,
          pan: true,
          scrollForPan: false,
          spacingHorizontal: 72,
          spacingVertical: 8,
          toggleRecursively: false,
          zoom: true
        });
        markmapRef.current = markmap;
        appliedScaleRef.current = 1;
        await markmap.setData(root);
        if (!active) return;
        await markmap.fit();
        const requestedScale = requestedScaleRef.current;
        if (requestedScale !== 1) {
          await markmap.rescale(requestedScale);
          appliedScaleRef.current = requestedScale;
        }
        if (active) setRenderState({ status: "ready" });
      } catch (error) {
        if (!active) return;
        markmap?.destroy();
        markmap = null;
        svg.replaceChildren();
        if (active) {
          setRenderState({ status: "error", message: renderErrorMessage(error) });
        }
      }
    })();

    return () => {
      active = false;
      if (markmapRef.current === markmap) markmapRef.current = null;
      markmap?.destroy();
      svg.replaceChildren();
    };
  }, [source, theme]);

  return (
    <div
      className={`thread-markmap-diagram ${theme === "dark" ? "markmap-dark" : ""}`}
      data-theme={theme}
      data-status={renderState.status}
      role="img"
      aria-label="Mind map"
    >
      <svg ref={svgRef} aria-hidden={renderState.status !== "ready"} />
      {renderState.status === "loading" ? (
        <div className="thread-markmap-loading" role="status">
          Rendering mind map...
        </div>
      ) : null}
      {renderState.status === "error" ? (
        <div className="thread-diagram-error">
          <p>Could not render mind map: {renderState.message}</p>
          <pre>
            <code>{source}</code>
          </pre>
        </div>
      ) : null}
    </div>
  );
});
