export type DiagramViewportMetrics = {
  clientWidth: number;
  clientHeight: number;
  scrollWidth: number;
  scrollHeight: number;
  scrollLeft: number;
  scrollTop: number;
};

export type DiagramViewportCenter = {
  x: number;
  y: number;
};

export const DIAGRAM_PAN_ACTIVATION_DISTANCE = 4;

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

export function diagramViewportCenter(
  viewport: DiagramViewportMetrics
): DiagramViewportCenter {
  return {
    x: clamp(
      (viewport.scrollLeft + viewport.clientWidth / 2) /
        Math.max(viewport.clientWidth, viewport.scrollWidth),
      0,
      1
    ),
    y: clamp(
      (viewport.scrollTop + viewport.clientHeight / 2) /
        Math.max(viewport.clientHeight, viewport.scrollHeight),
      0,
      1
    )
  };
}

export function diagramScrollForCenter(
  center: DiagramViewportCenter,
  viewport: Pick<
    DiagramViewportMetrics,
    "clientWidth" | "clientHeight" | "scrollWidth" | "scrollHeight"
  >
) {
  return {
    left: clamp(
      center.x * viewport.scrollWidth - viewport.clientWidth / 2,
      0,
      Math.max(0, viewport.scrollWidth - viewport.clientWidth)
    ),
    top: clamp(
      center.y * viewport.scrollHeight - viewport.clientHeight / 2,
      0,
      Math.max(0, viewport.scrollHeight - viewport.clientHeight)
    )
  };
}

export function diagramScrollForDrag(
  origin: { left: number; top: number; x: number; y: number },
  pointer: { x: number; y: number }
) {
  return {
    left: origin.left - (pointer.x - origin.x),
    top: origin.top - (pointer.y - origin.y)
  };
}

export function diagramViewportCanPan(
  viewport: Pick<
    DiagramViewportMetrics,
    "clientWidth" | "clientHeight" | "scrollWidth" | "scrollHeight"
  >
) {
  return (
    viewport.scrollWidth > viewport.clientWidth + 1 ||
    viewport.scrollHeight > viewport.clientHeight + 1
  );
}

export function diagramPanActivationReached(
  origin: { x: number; y: number },
  pointer: { x: number; y: number },
  threshold = DIAGRAM_PAN_ACTIVATION_DISTANCE
) {
  return Math.hypot(pointer.x - origin.x, pointer.y - origin.y) > threshold;
}
