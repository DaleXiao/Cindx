import {
  armVoiceCaptureDeadline,
  bytesToBase64,
  clearVoiceCaptureDeadline,
  resamplePcm16,
  type VoiceCaptureDeadline
} from "./voicePcmSupport";

const TARGET_SAMPLE_RATE = 16_000;
const MAX_RECORDING_SECONDS = 30;
const PROCESSOR_BUFFER_SIZE = 4_096;
const FIRST_AUDIO_FRAME_TIMEOUT_MS = 3_000;

export type VoicePcmCapture = VoiceCaptureDeadline & {
  audioContext: AudioContext;
  source: MediaStreamAudioSourceNode;
  processor: ScriptProcessorNode;
  chunks: Float32Array[];
  sampleCount: number;
  sampleRate: number;
  limitNotified: boolean;
};

export async function startVoicePcmCapture(
  onLimit: () => void
): Promise<VoicePcmCapture> {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      autoGainControl: true,
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true
    }
  });
  let audioContext: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let processor: ScriptProcessorNode | null = null;
  let firstFrameTimer: number | null = null;
  let resolveFirstFrame: (() => void) | null = null;
  try {
    audioContext = new AudioContext();
    source = audioContext.createMediaStreamSource(stream);
    processor = audioContext.createScriptProcessor(PROCESSOR_BUFFER_SIZE, 1, 1);
    const capture: VoicePcmCapture = {
      stream,
      audioContext,
      source,
      processor,
      chunks: [],
      sampleCount: 0,
      sampleRate: audioContext.sampleRate,
      limitNotified: false,
      limitTimer: null,
      trackEndedListener: null
    };
    const firstFrame = new Promise<void>((resolve, reject) => {
      resolveFirstFrame = resolve;
      firstFrameTimer = window.setTimeout(() => {
        firstFrameTimer = null;
        resolveFirstFrame = null;
        reject(new Error("Microphone capture did not start"));
      }, FIRST_AUDIO_FRAME_TIMEOUT_MS);
    });
    const maxSamples = Math.floor(capture.sampleRate * MAX_RECORDING_SECONDS);
    const notifyCaptureEnded = () => {
      if (capture.limitNotified) return;
      capture.limitNotified = true;
      window.setTimeout(onLimit, 0);
    };
    processor.onaudioprocess = (event) => {
      if (resolveFirstFrame) {
        const resolve = resolveFirstFrame;
        resolveFirstFrame = null;
        if (firstFrameTimer !== null) window.clearTimeout(firstFrameTimer);
        firstFrameTimer = null;
        resolve();
      }
      const input = event.inputBuffer.getChannelData(0);
      const remaining = maxSamples - capture.sampleCount;
      if (remaining <= 0) return;
      const accepted = input.slice(0, Math.min(input.length, remaining));
      capture.chunks.push(accepted);
      capture.sampleCount += accepted.length;
      if (capture.sampleCount >= maxSamples) notifyCaptureEnded();
    };
    source.connect(processor);
    processor.connect(audioContext.destination);
    await audioContext.resume();
    await firstFrame;
    armVoiceCaptureDeadline(capture, notifyCaptureEnded, MAX_RECORDING_SECONDS * 1_000);
    return capture;
  } catch (error) {
    if (firstFrameTimer !== null) window.clearTimeout(firstFrameTimer);
    firstFrameTimer = null;
    resolveFirstFrame = null;
    if (processor) processor.onaudioprocess = null;
    for (const node of [source, processor]) {
      try {
        node?.disconnect();
      } catch {
        // The node may not have reached a connected state.
      }
    }
    stream.getTracks().forEach((track) => {
      track.enabled = false;
      track.stop();
    });
    if (audioContext) await audioContext.close().catch(() => {});
    throw error;
  }
}

export function finishVoicePcmCapture(capture: VoicePcmCapture): string {
  const chunks = capture.chunks;
  const sampleCount = capture.sampleCount;
  const sampleRate = capture.sampleRate;
  disposeVoicePcmCapture(capture);
  const samples = new Float32Array(sampleCount);
  let offset = 0;
  for (const chunk of chunks) {
    samples.set(chunk, offset);
    offset += chunk.length;
  }
  return bytesToBase64(resamplePcm16(samples, sampleRate, TARGET_SAMPLE_RATE));
}

export function disposeVoicePcmCapture(capture: VoicePcmCapture) {
  clearVoiceCaptureDeadline(capture);
  capture.processor.onaudioprocess = null;
  capture.source.disconnect();
  capture.processor.disconnect();
  capture.stream.getTracks().forEach((track) => {
    track.enabled = false;
    track.stop();
  });
  void capture.audioContext.close().catch(() => {});
  capture.chunks = [];
  capture.sampleCount = 0;
}
