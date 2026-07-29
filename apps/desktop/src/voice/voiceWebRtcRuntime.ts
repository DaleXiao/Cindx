export type VoiceWebRtcResources = {
  peer: RTCPeerConnection | null;
  channel: RTCDataChannel | null;
  stream: MediaStream | null;
  audioContext: AudioContext | null;
  animationFrame: number | null;
  timeout: number | null;
  commitDelay: number | null;
  disconnectTimeout: number | null;
};

export function releaseVoiceCapture(run: VoiceWebRtcResources) {
  if (run.animationFrame !== null) window.cancelAnimationFrame(run.animationFrame);
  run.animationFrame = null;
  run.stream?.getTracks().forEach((track) => {
    track.enabled = false;
    track.stop();
  });
  run.stream = null;
  if (run.audioContext) void run.audioContext.close().catch(() => {});
  run.audioContext = null;
}

export function disposeVoiceWebRtc(run: VoiceWebRtcResources) {
  if (run.timeout !== null) window.clearTimeout(run.timeout);
  if (run.commitDelay !== null) window.clearTimeout(run.commitDelay);
  if (run.disconnectTimeout !== null) window.clearTimeout(run.disconnectTimeout);
  run.timeout = null;
  run.commitDelay = null;
  run.disconnectTimeout = null;
  releaseVoiceCapture(run);
  run.channel?.close();
  run.peer?.close();
  run.channel = null;
  run.peer = null;
}

export function updateVoiceDisconnectGrace(
  run: VoiceWebRtcResources,
  graceMs: number,
  onLost: () => void
) {
  const state = run.peer?.connectionState;
  if (state === "connected" && run.disconnectTimeout !== null) {
    window.clearTimeout(run.disconnectTimeout);
    run.disconnectTimeout = null;
  } else if (state === "disconnected" && run.disconnectTimeout === null) {
    run.disconnectTimeout = window.setTimeout(() => {
      run.disconnectTimeout = null;
      if (run.peer?.connectionState === "disconnected") onLost();
    }, graceMs);
  }
}

export function startVoiceOscilloscope(
  run: VoiceWebRtcResources,
  canvas: HTMLCanvasElement
) {
  if (!run.stream || run.audioContext) return;
  const audioContext = new AudioContext();
  const analyser = audioContext.createAnalyser();
  analyser.fftSize = 256;
  analyser.smoothingTimeConstant = 0.72;
  audioContext.createMediaStreamSource(run.stream).connect(analyser);
  void audioContext.resume().catch(() => {});
  run.audioContext = audioContext;

  const context = canvas.getContext("2d");
  if (!context) return;
  const samples = new Uint8Array(analyser.fftSize);
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  let previousFrame = 0;
  const draw = (timestamp: number) => {
    if (timestamp - previousFrame < 32) {
      run.animationFrame = window.requestAnimationFrame(draw);
      return;
    }
    previousFrame = timestamp;
    analyser.getByteTimeDomainData(samples);
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
    if (!reducedMotion) run.animationFrame = window.requestAnimationFrame(draw);
  };
  run.animationFrame = window.requestAnimationFrame(draw);
}
