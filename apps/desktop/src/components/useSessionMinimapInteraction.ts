import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent
} from "react";
import { MINIMAP_MARKER_GAP } from "./SessionThreadNavigation";

export function useSessionMinimapInteraction(
  markerCount: number,
  onSelectMarker: (index: number) => void,
  selectionEnabled: boolean
) {
  const minimapRef = useRef<HTMLDivElement>(null);
  const pointerRef = useRef<number | null>(null);
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null);
  const [previewIndex, setPreviewIndex] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);

  const indexFromPointer = useCallback(
    (clientY: number) => {
      const minimap = minimapRef.current;
      if (!minimap || markerCount === 0) return null;
      const bounds = minimap.getBoundingClientRect();
      const groupHeight = (markerCount - 1) * MINIMAP_MARKER_GAP;
      const groupStart = (bounds.height - groupHeight) / 2;
      const localY = clientY - bounds.top;
      if (
        localY < groupStart - MINIMAP_MARKER_GAP / 2 ||
        localY > groupStart + groupHeight + MINIMAP_MARKER_GAP / 2
      ) {
        return null;
      }
      const index = Math.round((localY - groupStart) / MINIMAP_MARKER_GAP);
      return Math.min(markerCount - 1, Math.max(0, index));
    },
    [markerCount]
  );

  const updateHover = useCallback(
    (clientY: number) => {
      const index = indexFromPointer(clientY);
      setHoveredIndex((current) => (current === index ? current : index));
    },
    [indexFromPointer]
  );

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!selectionEnabled) return;
      const markerIndex = indexFromPointer(event.clientY);
      if (markerIndex === null) return;
      event.preventDefault();
      pointerRef.current = event.pointerId;
      setDragging(true);
      setPreviewIndex(null);
      setHoveredIndex(markerIndex);
      event.currentTarget.setPointerCapture(event.pointerId);
      onSelectMarker(markerIndex);
    },
    [indexFromPointer, onSelectMarker, selectionEnabled]
  );

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (pointerRef.current === event.pointerId) {
        const markerIndex = indexFromPointer(event.clientY);
        if (markerIndex !== null) onSelectMarker(markerIndex);
        return;
      }
      updateHover(event.clientY);
    },
    [indexFromPointer, onSelectMarker, updateHover]
  );

  const handlePointerEnd = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (pointerRef.current !== event.pointerId) return;
      pointerRef.current = null;
      setDragging(false);
      updateHover(event.clientY);
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    },
    [updateHover]
  );

  const handlePointerCancel = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (pointerRef.current === event.pointerId) {
        pointerRef.current = null;
        setDragging(false);
      }
      setHoveredIndex(null);
      setPreviewIndex(null);
    },
    []
  );

  const handlePointerLeave = useCallback(() => {
    if (pointerRef.current !== null) return;
    setHoveredIndex(null);
    setPreviewIndex(null);
  }, []);

  const handlePointerEnter = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => updateHover(event.clientY),
    [updateHover]
  );

  useEffect(() => {
    setPreviewIndex(null);
    if (hoveredIndex === null || dragging) return;
    const timeout = window.setTimeout(() => setPreviewIndex(hoveredIndex), 420);
    return () => window.clearTimeout(timeout);
  }, [dragging, hoveredIndex]);

  return {
    handlePointerCancel,
    handlePointerDown,
    handlePointerEnd,
    handlePointerEnter,
    handlePointerLeave,
    handlePointerMove,
    hoveredIndex,
    minimapRef,
    previewIndex
  };
}
