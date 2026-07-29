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

export type VoicePcmCapture = VoiceCaptureDeadline & {
  audioContext: AudioContext;
  source: MediaStreamAudioSourceNode;
  analyser: AnalyserNode;
  processor: ScriptProcessorNode;
  sink: GainNode;
  chunks: Float32Array[];
  sampleCount: number;
  sampleRate: number;
  limitNotified: boolean;
  animationFrame: number | null;
};

export async function startVoicePcmCapture(
  canvas: HTMLCanvasElement | null,
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
  let analyser: AnalyserNode | null = null;
  let processor: ScriptProcessorNode | null = null;
  let sink: GainNode | null = null;
  try {
    audioContext = new AudioContext();
    source = audioContext.createMediaStreamSource(stream);
    analyser = audioContext.createAnalyser();
    processor = audioContext.createScriptProcessor(PROCESSOR_BUFFER_SIZE, 1, 1);
    sink = audioContext.createGain();
    const capture: VoicePcmCapture = {
      stream,
      audioContext,
      source,
      analyser,
      processor,
      sink,
      chunks: [],
      sampleCount: 0,
      sampleRate: audioContext.sampleRate,
      limitNotified: false,
      limitTimer: null,
      trackEndedListener: null,
      animationFrame: null
    };
    const maxSamples = Math.floor(capture.sampleRate * MAX_RECORDING_SECONDS);
    const notifyCaptureEnded = () => {
      if (capture.limitNotified) return;
      capture.limitNotified = true;
      window.setTimeout(onLimit, 0);
    };
    analyser.fftSize = 256;
    analyser.smoothingTimeConstant = 0.72;
    sink.gain.value = 0;
    processor.onaudioprocess = (event) => {
      const input = event.inputBuffer.getChannelData(0);
      const remaining = maxSamples - capture.sampleCount;
      if (remaining <= 0) return;
      const accepted = input.slice(0, Math.min(input.length, remaining));
      capture.chunks.push(accepted);
      capture.sampleCount += accepted.length;
      if (capture.sampleCount >= maxSamples) notifyCaptureEnded();
    };
    source.connect(analyser);
    analyser.connect(processor);
    processor.connect(sink);
    sink.connect(audioContext.destination);
    await audioContext.resume();
    armVoiceCaptureDeadline(capture, notifyCaptureEnded, MAX_RECORDING_SECONDS * 1_000);
    if (canvas) startOscilloscope(capture, canvas);
    return capture;
  } catch (error) {
    if (processor) processor.onaudioprocess = null;
    for (const node of [source, analyser, processor, sink]) {
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
  if (capture.animationFrame !== null) window.cancelAnimationFrame(capture.animationFrame);
  capture.animationFrame = null;
  capture.processor.onaudioprocess = null;
  capture.source.disconnect();
  capture.analyser.disconnect();
  capture.processor.disconnect();
  capture.sink.disconnect();
  capture.stream.getTracks().forEach((track) => {
    track.enabled = false;
    track.stop();
  });
  void capture.audioContext.close().catch(() => {});
  capture.chunks = [];
  capture.sampleCount = 0;
}

function startOscilloscope(capture: VoicePcmCapture, canvas: HTMLCanvasElement) {
  const context = canvas.getContext("2d");
  if (!context) return;
  const samples = new Uint8Array(capture.analyser.fftSize);
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  let previousFrame = 0;
  const draw = (timestamp: number) => {
    if (timestamp - previousFrame < 32) {
      capture.animationFrame = window.requestAnimationFrame(draw);
      return;
    }
    previousFrame = timestamp;
    capture.analyser.getByteTimeDomainData(samples);
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.strokeStyle = "#ffffff";
    context.lineWidth = 3;
    context.lineCap = "round";
    context.beginPath();
    samples.forEach((sample, index) => {
      const x = (index / (samples.length - 1)) * canvas.width;
      const y = reducedMotion
        ? canvas.height / 2
        : (sample / 255) * canvas.height * 0.7 + canvas.height * 0.15;
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.stroke();
    if (!reducedMotion) capture.animationFrame = window.requestAnimationFrame(draw);
  };
  capture.animationFrame = window.requestAnimationFrame(draw);
}
