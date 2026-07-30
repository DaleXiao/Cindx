export type VoiceWebRtcResources = {
  peer: RTCPeerConnection | null;
  channel: RTCDataChannel | null;
  stream: MediaStream | null;
  timeout: number | null;
  commitDelay: number | null;
  disconnectTimeout: number | null;
};

export function releaseVoiceCapture(run: VoiceWebRtcResources) {
  run.stream?.getTracks().forEach((track) => {
    track.enabled = false;
    track.stop();
  });
  run.stream = null;
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
