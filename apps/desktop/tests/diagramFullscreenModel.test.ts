import assert from "node:assert/strict";
import test from "node:test";
import {
  DIAGRAM_PAN_ACTIVATION_DISTANCE,
  diagramPanActivationReached,
  diagramScrollForCenter,
  diagramScrollForDrag,
  diagramViewportCenter,
  diagramViewportCanPan
} from "../src/components/diagramFullscreenModel.ts";

test("diagram zoom preserves the normalized viewport center", () => {
  const center = diagramViewportCenter({
    clientWidth: 100,
    clientHeight: 80,
    scrollWidth: 300,
    scrollHeight: 240,
    scrollLeft: 50,
    scrollTop: 40
  });

  assert.deepEqual(center, { x: 1 / 3, y: 1 / 3 });
  assert.deepEqual(
    diagramScrollForCenter(center, {
      clientWidth: 100,
      clientHeight: 80,
      scrollWidth: 600,
      scrollHeight: 480
    }),
    { left: 150, top: 120 }
  );
});

test("diagram center and drag positions stay bounded by native scrolling", () => {
  assert.deepEqual(
    diagramScrollForCenter(
      { x: 1, y: 0 },
      { clientWidth: 100, clientHeight: 80, scrollWidth: 240, scrollHeight: 160 }
    ),
    { left: 140, top: 0 }
  );
  assert.deepEqual(
    diagramScrollForDrag(
      { left: 90, top: 70, x: 200, y: 160 },
      { x: 170, y: 190 }
    ),
    { left: 120, top: 40 }
  );
});

test("diagram panning is available only when the viewport actually overflows", () => {
  assert.equal(
    diagramViewportCanPan({
      clientWidth: 100,
      clientHeight: 80,
      scrollWidth: 101,
      scrollHeight: 81
    }),
    false
  );
  assert.equal(
    diagramViewportCanPan({
      clientWidth: 100,
      clientHeight: 80,
      scrollWidth: 102,
      scrollHeight: 80
    }),
    true
  );
});

test("diagram drag activation leaves ordinary clicks and small selections alone", () => {
  const origin = { x: 20, y: 30 };
  assert.equal(
    diagramPanActivationReached(
      origin,
      { x: 20 + DIAGRAM_PAN_ACTIVATION_DISTANCE, y: 30 }
    ),
    false
  );
  assert.equal(diagramPanActivationReached(origin, { x: 23, y: 34 }), true);
});
