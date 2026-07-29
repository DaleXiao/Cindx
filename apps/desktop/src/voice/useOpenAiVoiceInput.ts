import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { negotiateVoiceSession } from "../tauri";
import {
  parseVoiceServerEvent,
  reduceVoiceTurnEvent,
  voiceStopAction,
  type VoiceInputStatus,
  type VoiceTurnState
} from "./voiceInputModel";
import {
  disposeVoiceWebRtc,
  releaseVoiceCapture,
  startVoiceOscilloscope,
  updateVoiceDisconnectGrace,
  type VoiceWebRtcResources
} from "./voiceWebRtcRuntime";

const VOICE_CONNECT_TIMEOUT_MS = 30_000;
const VOICE_FINISH_TIMEOUT_MS = 10_000;
const VOICE_COMMIT_DRAIN_MS = 75;
const VOICE_DISCONNECT_GRACE_MS = 3_000;
type VoiceRun = VoiceWebRtcResources & {
  id: number;
  sessionId: string;
  turn: VoiceTurnState;
};

type UseVoiceInputOptions = {
  enabled: boolean;
  sessionId: string | null;
  onTranscript: (sessionId: string, text: string) => void;
  onError: (message: string) => void;
  onStatusChange: (status: VoiceInputStatus) => void;
};

function errorMessage(error: unknown): string {
  if (error instanceof DOMException) {
    if (error.name === "NotAllowedError") return "Microphone access was denied";
    if (error.name === "NotFoundError") return "No microphone is available";
  }
  return error instanceof Error ? error.message : String(error);
}

export function useOpenAiVoiceInput({
  enabled,
  sessionId,
  onTranscript,
  onError,
  onStatusChange
}: UseVoiceInputOptions) {
  const [status, setStatus] = useState<VoiceInputStatus>("idle");
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const activeRunRef = useRef<VoiceRun | null>(null);
  const runSequenceRef = useRef(0);
  const sessionIdRef = useRef(sessionId);
  const transcriptCallbackRef = useRef(onTranscript);
  const errorCallbackRef = useRef(onError);
  const statusCallbackRef = useRef(onStatusChange);
  transcriptCallbackRef.current = onTranscript;
  errorCallbackRef.current = onError;
  statusCallbackRef.current = onStatusChange;
  sessionIdRef.current = sessionId;

  const publishStatus = useCallback((next: VoiceInputStatus) => {
    setStatus(next);
    statusCallbackRef.current(next);
  }, []);

  const isCurrent = useCallback(
    (run: VoiceRun) =>
      activeRunRef.current === run &&
      runSequenceRef.current === run.id &&
      sessionIdRef.current === run.sessionId,
    []
  );

  const forceDispose = useCallback((updateStatus = true) => {
    runSequenceRef.current += 1;
    const run = activeRunRef.current;
    activeRunRef.current = null;
    if (run) disposeVoiceWebRtc(run);
    if (updateStatus) publishStatus("idle");
  }, [publishStatus]);

  const failRun = useCallback(
    (run: VoiceRun, message: string) => {
      if (!isCurrent(run)) return;
      errorCallbackRef.current(message);
      forceDispose();
    },
    [forceDispose, isCurrent]
  );

  const completeRun = useCallback(
    (run: VoiceRun, transcript: string) => {
      if (!isCurrent(run)) return;
      activeRunRef.current = null;
      disposeVoiceWebRtc(run);
      publishStatus("idle");
      if (transcript) transcriptCallbackRef.current(run.sessionId, transcript);
    },
    [isCurrent, publishStatus]
  );

  const sendCommit = useCallback(
    (run: VoiceRun) => {
      if (
        !isCurrent(run) ||
        run.turn.status !== "finishing" ||
        run.turn.commitSent ||
        run.channel?.readyState !== "open"
      ) {
        return;
      }
      try {
        run.channel.send(JSON.stringify({ type: "input_audio_buffer.commit" }));
        run.turn = { ...run.turn, commitSent: true };
      } catch (error) {
        failRun(run, errorMessage(error));
      }
    },
    [failRun, isCurrent]
  );

  const beginFinishing = useCallback(
    (run: VoiceRun) => {
      if (!isCurrent(run) || voiceStopAction(run.turn.status) !== "finish") return;
      run.turn = { ...run.turn, status: "finishing" };
      releaseVoiceCapture(run);
      publishStatus("finishing");
      if (run.timeout !== null) window.clearTimeout(run.timeout);
      run.timeout = window.setTimeout(
        () => failRun(run, "Voice transcription timed out"),
        VOICE_FINISH_TIMEOUT_MS
      );
      run.commitDelay = window.setTimeout(() => {
        run.commitDelay = null;
        sendCommit(run);
      }, VOICE_COMMIT_DRAIN_MS);
    },
    [failRun, isCurrent, publishStatus, sendCommit]
  );

  const start = useCallback(async () => {
    if (!enabled || !sessionId || activeRunRef.current) return;
    if (!navigator.mediaDevices?.getUserMedia || typeof RTCPeerConnection === "undefined") {
      errorCallbackRef.current("Voice input is unavailable in this runtime");
      return;
    }
    const run: VoiceRun = {
      id: ++runSequenceRef.current,
      sessionId,
      turn: {
        status: "connecting",
        commitSent: false,
        committedItemId: null,
        seenItemIds: []
      },
      peer: null,
      channel: null,
      stream: null,
      audioContext: null,
      animationFrame: null,
      timeout: null,
      commitDelay: null,
      disconnectTimeout: null
    };
    activeRunRef.current = run;
    publishStatus("connecting");
    try {
      run.stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          autoGainControl: true,
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true
        }
      });
      if (!isCurrent(run)) return disposeVoiceWebRtc(run);
      run.timeout = window.setTimeout(
        () => failRun(run, "Voice connection timed out"),
        VOICE_CONNECT_TIMEOUT_MS
      );
      run.peer = new RTCPeerConnection();
      run.stream.getAudioTracks().forEach((track) => {
        track.enabled = false;
        run.peer?.addTrack(track, run.stream as MediaStream);
        track.addEventListener("ended", () => {
          if (!isCurrent(run)) return;
          const action = voiceStopAction(run.turn.status);
          if (action === "finish") beginFinishing(run);
          else if (action === "discard") forceDispose();
        });
      });
      run.channel = run.peer.createDataChannel("oai-events");
      run.channel.addEventListener("open", () => {
        if (!isCurrent(run)) return;
        if (run.turn.status === "finishing") return sendCommit(run);
        if (run.turn.status !== "connecting") return;
        try {
          run.channel?.send(JSON.stringify({ type: "input_audio_buffer.clear" }));
          run.stream?.getAudioTracks().forEach((track) => {
            track.enabled = true;
          });
          if (run.timeout !== null) window.clearTimeout(run.timeout);
          run.timeout = null;
          run.turn = { ...run.turn, status: "recording" };
          if (canvasRef.current) startVoiceOscilloscope(run, canvasRef.current);
          publishStatus("recording");
        } catch (error) {
          failRun(run, errorMessage(error));
        }
      });
      run.channel.addEventListener("message", (message) => {
        if (!isCurrent(run) || typeof message.data !== "string") return;
        const event = parseVoiceServerEvent(message.data);
        if (!event) return;
        if (event.kind === "error") return failRun(run, event.message);
        const result = reduceVoiceTurnEvent(run.turn, event);
        run.turn = result.state;
        if (result.completed) completeRun(run, result.transcript);
      });
      run.channel.addEventListener("close", () => {
        if (isCurrent(run)) failRun(run, "Voice connection closed");
      });
      run.peer.addEventListener("connectionstatechange", () => {
        if (!isCurrent(run)) return;
        const connectionState = run.peer?.connectionState;
        if (connectionState === "failed" || connectionState === "closed") {
          return failRun(run, "Voice connection failed");
        }
        updateVoiceDisconnectGrace(run, VOICE_DISCONNECT_GRACE_MS, () => {
          if (isCurrent(run)) failRun(run, "Voice connection was lost");
        });
      });
      const offer = await run.peer.createOffer();
      await run.peer.setLocalDescription(offer);
      if (!isCurrent(run)) return;
      if (!offer.sdp) throw new Error("Voice connection did not produce an SDP offer");
      const answer = await negotiateVoiceSession(offer.sdp);
      if (!isCurrent(run)) return;
      await run.peer.setRemoteDescription({ type: "answer", sdp: answer.answerSdp });
    } catch (error) {
      failRun(run, errorMessage(error));
    }
  }, [
    beginFinishing,
    completeRun,
    enabled,
    failRun,
    forceDispose,
    isCurrent,
    publishStatus,
    sendCommit,
    sessionId
  ]);

  const toggle = useCallback(() => {
    const run = activeRunRef.current;
    if (!run) return void start();
    const action = voiceStopAction(run.turn.status);
    if (action === "discard") forceDispose();
    else if (action === "finish") beginFinishing(run);
  }, [beginFinishing, forceDispose, start]);

  useEffect(() => {
    if (!enabled) forceDispose();
  }, [enabled, forceDispose]);
  useLayoutEffect(() => {
    const run = activeRunRef.current;
    if (run && run.sessionId !== sessionId) forceDispose();
  }, [forceDispose, sessionId]);
  useEffect(() => () => forceDispose(false), [forceDispose]);

  return { canvasRef, status, toggle };
}
