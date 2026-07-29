import assert from "node:assert/strict";
import test from "node:test";
import {
  armVoiceCaptureDeadline,
  bytesToBase64,
  clearVoiceCaptureDeadline,
  resamplePcm16
} from "../src/voice/voicePcmSupport.ts";

test("voice capture deadline observes both track end and wall time", () => {
  let endedListener: (() => void) | null = null;
  let scheduled: (() => void) | null = null;
  let notifications = 0;
  const capture = {
    limitTimer: null,
    trackEndedListener: null,
    stream: {
      getAudioTracks: () => [{
        readyState: "live",
        addEventListener: (_type: string, callback: () => void) => { endedListener = callback; }
      }]
    } as unknown as MediaStream
  };

  armVoiceCaptureDeadline(capture, () => { notifications += 1; }, 30_000, (callback, delay) => {
    assert.equal(delay, 30_000);
    scheduled = callback;
    return 42;
  });

  assert.equal(capture.limitTimer, 42);
  assert.equal(capture.trackEndedListener, endedListener);
  assert.ok(endedListener);
  assert.ok(scheduled);
  (endedListener as () => void)();
  (scheduled as () => void)();
  assert.equal(notifications, 2);
});

test("PCM capture output is little-endian signed 16-bit audio", () => {
  const bytes = resamplePcm16(new Float32Array([-1, 0, 1]), 16_000, 16_000);
  assert.deepEqual([...bytes], [0x00, 0x80, 0x00, 0x00, 0xff, 0x7f]);
});

test("PCM resampling converts one second of 48kHz mono audio to bounded 16kHz", () => {
  const input = new Float32Array(48_000);
  const bytes = resamplePcm16(input, 48_000, 16_000);
  assert.equal(bytes.length, 16_000 * 2);
});

test("PCM resampling rejects unusable source rates without allocating output", () => {
  assert.equal(resamplePcm16(new Float32Array([0.5]), 0, 16_000).length, 0);
  assert.equal(resamplePcm16(new Float32Array(), 48_000, 16_000).length, 0);
});

test("PCM bytes survive the frontend base64 transport without mutation", () => {
  const bytes = new Uint8Array([0x00, 0x80, 0xff, 0x7f, 0x24, 0x00]);
  const decoded = Buffer.from(bytesToBase64(bytes), "base64");
  assert.deepEqual([...decoded], [...bytes]);
});

test("voice capture cleanup clears its wall timer and track listener exactly once", () => {
  const listener = () => {};
  const cleared: number[] = [];
  const removed: Array<[string, () => void]> = [];
  const capture = {
    limitTimer: 42,
    trackEndedListener: listener,
    stream: {
      getAudioTracks: () => [
        {
          removeEventListener: (type: string, callback: () => void) => {
            removed.push([type, callback]);
          }
        }
      ]
    } as unknown as MediaStream
  };

  clearVoiceCaptureDeadline(capture, (timer) => cleared.push(timer));
  clearVoiceCaptureDeadline(capture, (timer) => cleared.push(timer));

  assert.deepEqual(cleared, [42]);
  assert.deepEqual(removed, [["ended", listener]]);
  assert.equal(capture.limitTimer, null);
  assert.equal(capture.trackEndedListener, null);
});
