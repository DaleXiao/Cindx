import { Download, RotateCcw, X, ZoomIn, ZoomOut } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { MarkdownDiagram } from "./markdownDiagramModel";
import { MarkmapDiagram } from "./MarkmapDiagram";
import { MermaidDiagram } from "./MermaidDiagram";
import { useResolvedTheme } from "./useResolvedTheme";

type DiagramFullscreenProps = {
  diagram: MarkdownDiagram;
  onClose: () => void;
  onError: (message: string) => void;
};

const MIN_ZOOM = 0.6;
const MAX_ZOOM = 2;
const ZOOM_STEP = 0.2;
const MAX_EXPORT_DIMENSION = 4_096;

function loadSvgImage(source: string) {
  return new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("Could not prepare the diagram image"));
    image.src = source;
  });
}

function canvasPng(canvas: HTMLCanvasElement) {
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("Could not encode the diagram as PNG"));
    }, "image/png");
  });
}

function replaceForeignObjectsWithSvgText(source: SVGSVGElement, clone: SVGSVGElement) {
  const sourceLabels = Array.from(source.querySelectorAll("foreignObject"));
  const clonedLabels = Array.from(clone.querySelectorAll("foreignObject"));

  clonedLabels.forEach((clonedLabel, index) => {
    const sourceLabel = sourceLabels[index];
    if (!sourceLabel) return;
    const htmlLabel = sourceLabel.querySelector<HTMLElement>("div, span, p");
    const rawText = htmlLabel?.innerText || sourceLabel.textContent || "";
    const lines = rawText
      .split(/\n+/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (lines.length === 0) {
      clonedLabel.remove();
      return;
    }

    const bounds = sourceLabel.getBBox();
    const width = Number.parseFloat(clonedLabel.getAttribute("width") || "") || bounds.width;
    const height = Number.parseFloat(clonedLabel.getAttribute("height") || "") || bounds.height;
    const computed = getComputedStyle(htmlLabel || sourceLabel);
    const fontSize = Number.parseFloat(computed.fontSize) || 14;
    const lineHeight = Number.parseFloat(computed.lineHeight) || fontSize * 1.4;
    const firstLineY = height / 2 - ((lines.length - 1) * lineHeight) / 2;
    const text = document.createElementNS("http://www.w3.org/2000/svg", "text");
    text.setAttribute("x", String(width / 2));
    text.setAttribute("y", String(firstLineY));
    text.setAttribute("fill", computed.color);
    text.setAttribute("text-anchor", "middle");
    text.setAttribute("dominant-baseline", "middle");
    text.setAttribute("font-family", computed.fontFamily);
    text.setAttribute("font-size", computed.fontSize || `${fontSize}px`);
    text.setAttribute("font-style", computed.fontStyle);
    text.setAttribute("font-weight", computed.fontWeight);

    lines.forEach((line, lineIndex) => {
      const span = document.createElementNS("http://www.w3.org/2000/svg", "tspan");
      span.setAttribute("x", String(width / 2));
      if (lineIndex > 0) span.setAttribute("dy", String(lineHeight));
      span.textContent = line;
      text.append(span);
    });
    clonedLabel.replaceWith(text);
  });
}

async function downloadDiagramPng(
  container: HTMLElement,
  diagram: MarkdownDiagram,
  theme: "light" | "dark"
) {
  const svg = container.querySelector("svg");
  if (!svg) throw new Error("The diagram is not ready yet");

  const bounds = svg.getBoundingClientRect();
  const width = Math.max(1, Math.ceil(bounds.width));
  const height = Math.max(1, Math.ceil(bounds.height));
  const clone = svg.cloneNode(true) as SVGSVGElement;
  clone.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  clone.setAttribute("xmlns:xlink", "http://www.w3.org/1999/xlink");
  clone.setAttribute("width", String(width));
  clone.setAttribute("height", String(height));
  if (!clone.hasAttribute("viewBox")) clone.setAttribute("viewBox", `0 0 ${width} ${height}`);
  clone.style.color = theme === "dark" ? "#f1f1f1" : "#181818";
  clone.style.setProperty("--markmap-text-color", theme === "dark" ? "#f4f4f4" : "#252525");
  clone.style.setProperty("--markmap-code-bg", theme === "dark" ? "#292929" : "#f0f0f0");
  clone.style.setProperty("--markmap-code-color", theme === "dark" ? "#f1f1f1" : "#555555");
  clone.style.setProperty("--markmap-circle-open-bg", theme === "dark" ? "#202020" : "#ffffff");
  replaceForeignObjectsWithSvgText(svg, clone);

  const serialized = new XMLSerializer().serializeToString(clone);
  const svgUrl = URL.createObjectURL(
    new Blob([serialized], { type: "image/svg+xml;charset=utf-8" })
  );
  try {
    const image = await loadSvgImage(svgUrl);
    const outputScale = Math.max(
      0.25,
      Math.min(2, MAX_EXPORT_DIMENSION / width, MAX_EXPORT_DIMENSION / height)
    );
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(width * outputScale));
    canvas.height = Math.max(1, Math.round(height * outputScale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("PNG export is unavailable on this device");
    context.scale(outputScale, outputScale);
    context.fillStyle = theme === "dark" ? "#171717" : "#ffffff";
    context.fillRect(0, 0, width, height);
    context.drawImage(image, 0, 0, width, height);

    const png = await canvasPng(canvas);
    const pngUrl = URL.createObjectURL(png);
    const link = document.createElement("a");
    const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
    link.href = pngUrl;
    link.download = `cindx-${diagram.kind}-${timestamp}.png`;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(pngUrl), 1_000);
  } finally {
    URL.revokeObjectURL(svgUrl);
  }
}

export function DiagramFullscreen({ diagram, onClose, onError }: DiagramFullscreenProps) {
  const theme = useResolvedTheme();
  const surfaceRef = useRef<HTMLDivElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const [zoom, setZoom] = useState(1);
  const [downloading, setDownloading] = useState(false);

  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    closeButtonRef.current?.focus({ preventScroll: true });
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      document.body.style.overflow = previousOverflow;
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [onClose]);

  const adjustZoom = useCallback((delta: number) => {
    setZoom((current) =>
      Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Number((current + delta).toFixed(1))))
    );
  }, []);

  const download = useCallback(async () => {
    const surface = surfaceRef.current;
    if (!surface || downloading) return;
    setDownloading(true);
    try {
      await downloadDiagramPng(surface, diagram, theme);
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error));
    } finally {
      setDownloading(false);
    }
  }, [diagram, downloading, onError, theme]);

  return createPortal(
    <section
      className="thread-diagram-fullscreen"
      role="dialog"
      aria-modal="true"
      aria-label={diagram.kind === "mindmap" ? "Mind map fullscreen" : "Mermaid fullscreen"}
    >
      <div className="thread-diagram-fullscreen-actions">
        <button
          type="button"
          aria-label="Download diagram as PNG"
          title="Download PNG"
          disabled={downloading}
          onClick={() => void download()}
        >
          <Download aria-hidden="true" />
        </button>
        <button
          ref={closeButtonRef}
          type="button"
          aria-label="Close fullscreen diagram"
          title="Close"
          onClick={onClose}
        >
          <X aria-hidden="true" />
        </button>
      </div>
      <div className="thread-diagram-fullscreen-viewport">
        <div
          className="thread-diagram-fullscreen-surface"
          ref={surfaceRef}
          style={
            diagram.kind === "mindmap"
              ? { width: "100%", height: "100%" }
              : { width: `${zoom * 100}%`, height: `${zoom * 100}%` }
          }
        >
          {diagram.kind === "mindmap" ? (
            <MarkmapDiagram source={diagram.source} scale={zoom} />
          ) : (
            <MermaidDiagram source={diagram.source} />
          )}
        </div>
      </div>
      <div className="thread-diagram-zoom-controls" aria-label="Diagram zoom controls">
        <button
          type="button"
          aria-label="Zoom out"
          title="Zoom out"
          disabled={zoom <= MIN_ZOOM}
          onClick={() => adjustZoom(-ZOOM_STEP)}
        >
          <ZoomOut aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Reset zoom"
          title="Reset zoom"
          disabled={zoom === 1}
          onClick={() => setZoom(1)}
        >
          <RotateCcw aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Zoom in"
          title="Zoom in"
          disabled={zoom >= MAX_ZOOM}
          onClick={() => adjustZoom(ZOOM_STEP)}
        >
          <ZoomIn aria-hidden="true" />
        </button>
      </div>
    </section>,
    document.body
  );
}
