import type {
  KeyboardEventHandler,
  PointerEventHandler,
  RefObject
} from "react";
import type { MinimapMarker } from "./sessionThreadProjection";
import { minimapMarkerPosition } from "./SessionThreadNavigation";

type SessionMinimapProps = {
  available: boolean;
  hoveredIndex: number | null;
  markers: MinimapMarker[];
  minimapRef: RefObject<HTMLDivElement | null>;
  onKeyDown: KeyboardEventHandler<HTMLDivElement>;
  onPointerCancel: PointerEventHandler<HTMLDivElement>;
  onPointerDown: PointerEventHandler<HTMLDivElement>;
  onPointerEnter: PointerEventHandler<HTMLDivElement>;
  onPointerLeave: PointerEventHandler<HTMLDivElement>;
  onPointerMove: PointerEventHandler<HTMLDivElement>;
  onPointerUp: PointerEventHandler<HTMLDivElement>;
  positionIndex: number;
  previewIndex: number | null;
  scrollRange: number;
  scrollTop: number;
};

export function SessionMinimap({
  available,
  hoveredIndex,
  markers,
  minimapRef,
  onKeyDown,
  onPointerCancel,
  onPointerDown,
  onPointerEnter,
  onPointerLeave,
  onPointerMove,
  onPointerUp,
  positionIndex,
  previewIndex,
  scrollRange,
  scrollTop
}: SessionMinimapProps) {
  const preview = previewIndex === null ? null : markers[previewIndex] ?? null;
  return (
    <div
      className="thread-minimap"
      data-scrollable={available}
      ref={minimapRef}
      role="scrollbar"
      aria-label="Navigate conversation"
      aria-controls="session-thread-scroll"
      aria-orientation="vertical"
      aria-valuemin={0}
      aria-valuemax={Math.round(scrollRange)}
      aria-valuenow={Math.round(scrollTop)}
      tabIndex={available ? 0 : -1}
      onKeyDown={onKeyDown}
      onPointerEnter={onPointerEnter}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onPointerLeave={onPointerLeave}
    >
      <div className="thread-minimap-track" aria-hidden="true">
        {markers.map((marker, index) => {
          const waveDistance = hoveredIndex === null ? null : Math.abs(index - hoveredIndex);
          return (
            <span
              className={`thread-minimap-marker thread-minimap-marker-${marker.kind}`}
              data-wave-distance={
                waveDistance !== null && waveDistance <= 2 ? waveDistance : undefined
              }
              data-edge-fade={index < 3 ? index : undefined}
              key={marker.id}
              style={{ top: minimapMarkerPosition(index, markers.length) }}
            />
          );
        })}
        <span
          className="thread-minimap-position"
          style={{ top: minimapMarkerPosition(positionIndex, markers.length) }}
        />
      </div>
      {preview && (
        <aside
          className="thread-minimap-preview"
          role="tooltip"
          style={{
            top: `clamp(48px, ${minimapMarkerPosition(
              previewIndex ?? 0,
              markers.length
            )}, calc(100% - 48px))`
          }}
        >
          <header>
            <strong>{preview.label}</strong>
          </header>
          <p>{preview.preview}</p>
        </aside>
      )}
    </div>
  );
}
