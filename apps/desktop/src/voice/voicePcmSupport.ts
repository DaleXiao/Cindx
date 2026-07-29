export type VoiceCaptureDeadline = {
  stream: MediaStream;
  limitTimer: number | null;
  trackEndedListener: (() => void) | null;
};

export function armVoiceCaptureDeadline(
  capture: VoiceCaptureDeadline,
  notify: () => void,
  limitMs: number,
  scheduleTimer: (callback: () => void, delay: number) => number = (callback, delay) =>
    window.setTimeout(callback, delay)
) {
  capture.trackEndedListener = notify;
  capture.stream.getAudioTracks().forEach((track) => {
    track.addEventListener("ended", notify);
    if (track.readyState === "ended") notify();
  });
  capture.limitTimer = scheduleTimer(notify, limitMs);
}

export function clearVoiceCaptureDeadline(
  capture: VoiceCaptureDeadline,
  clearTimer: (timer: number) => void = (timer) => window.clearTimeout(timer)
) {
  if (capture.limitTimer !== null) clearTimer(capture.limitTimer);
  capture.limitTimer = null;
  const listener = capture.trackEndedListener;
  if (listener) {
    capture.stream.getAudioTracks().forEach((track) => {
      track.removeEventListener("ended", listener);
    });
  }
  capture.trackEndedListener = null;
}

export function resamplePcm16(
  input: Float32Array,
  sourceRate: number,
  targetRate: number
): Uint8Array {
  if (!input.length || sourceRate <= 0 || targetRate <= 0) return new Uint8Array();
  const outputLength = Math.floor((input.length * targetRate) / sourceRate);
  const bytes = new Uint8Array(outputLength * 2);
  const view = new DataView(bytes.buffer);
  const ratio = sourceRate / targetRate;
  for (let index = 0; index < outputLength; index += 1) {
    const position = index * ratio;
    const left = Math.min(input.length - 1, Math.floor(position));
    const right = Math.min(input.length - 1, left + 1);
    const fraction = position - left;
    const sample = Math.max(-1, Math.min(1, input[left] * (1 - fraction) + input[right] * fraction));
    const pcm = sample < 0 ? Math.round(sample * 0x8000) : Math.round(sample * 0x7fff);
    view.setInt16(index * 2, pcm, true);
  }
  return bytes;
}

export function bytesToBase64(bytes: Uint8Array): string {
  const parts: string[] = [];
  const chunkSize = 32_768;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    parts.push(String.fromCharCode(...bytes.subarray(offset, offset + chunkSize)));
  }
  return btoa(parts.join(""));
}
