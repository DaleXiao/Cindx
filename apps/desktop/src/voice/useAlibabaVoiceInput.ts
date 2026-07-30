import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { transcribeVoiceAudio } from "../tauri";
import { voiceWatchdogAction, type VoiceInputStatus } from "./voiceInputModel";
import {
  disposeVoicePcmCapture,
  finishVoicePcmCapture,
  startVoicePcmCapture,
  type VoicePcmCapture
} from "./voicePcmRuntime";

type AlibabaVoiceRun = {
  id: number;
  sessionId: string;
  status: VoiceInputStatus;
  capture: VoicePcmCapture | null;
  finishingTimeout: number | null;
};

const VOICE_TRANSCRIPTION_TIMEOUT_MS = 60_000;

type UseAlibabaVoiceInputOptions = {
  enabled: boolean;
  sessionId: string | null;
  onTranscript: (sessionId: string, text: string) => void;
  onError: (message: string) => void;
  onStatusChange: (status: VoiceInputStatus) => void;
};

function errorMessage(error: unknown): string {
  if (error instanceof DOMException) {
    if (error.name === "NotAllowedError") {
      return "Microphone access was denied. Enable Cindx in System Settings → Privacy & Security → Microphone.";
    }
    if (error.name === "NotFoundError") return "No microphone is available";
    if (error.name === "NotReadableError") return "The microphone is busy or unavailable";
  }
  return error instanceof Error ? error.message : String(error);
}

export function useAlibabaVoiceInput({
  enabled,
  sessionId,
  onTranscript,
  onError,
  onStatusChange
}: UseAlibabaVoiceInputOptions) {
  const [status, setStatus] = useState<VoiceInputStatus>("idle");
  const activeRunRef = useRef<AlibabaVoiceRun | null>(null);
  const runSequenceRef = useRef(0);
  const sessionIdRef = useRef(sessionId);
  const transcriptCallbackRef = useRef(onTranscript);
  const errorCallbackRef = useRef(onError);
  const statusCallbackRef = useRef(onStatusChange);
  sessionIdRef.current = sessionId;
  transcriptCallbackRef.current = onTranscript;
  errorCallbackRef.current = onError;
  statusCallbackRef.current = onStatusChange;

  const publishStatus = useCallback((next: VoiceInputStatus) => {
    setStatus(next);
    statusCallbackRef.current(next);
  }, []);

  const isCurrent = useCallback(
    (run: AlibabaVoiceRun) =>
      activeRunRef.current === run &&
      runSequenceRef.current === run.id &&
      sessionIdRef.current === run.sessionId,
    []
  );

  const forceDispose = useCallback(
    (updateStatus = true) => {
      runSequenceRef.current += 1;
      const run = activeRunRef.current;
      activeRunRef.current = null;
      if (run && run.finishingTimeout !== null) {
        window.clearTimeout(run.finishingTimeout);
        run.finishingTimeout = null;
      }
      if (run?.capture) disposeVoicePcmCapture(run.capture);
      if (updateStatus) publishStatus("idle");
    },
    [publishStatus]
  );

  const failRun = useCallback(
    (run: AlibabaVoiceRun, error: unknown) => {
      if (!isCurrent(run)) return;
      errorCallbackRef.current(errorMessage(error));
      forceDispose();
    },
    [forceDispose, isCurrent]
  );

  const completeRun = useCallback(
    (run: AlibabaVoiceRun, transcript: string) => {
      if (!isCurrent(run)) return;
      activeRunRef.current = null;
      if (run.finishingTimeout !== null) window.clearTimeout(run.finishingTimeout);
      run.finishingTimeout = null;
      publishStatus("idle");
      if (transcript.trim()) transcriptCallbackRef.current(run.sessionId, transcript.trim());
    },
    [isCurrent, publishStatus]
  );

  const finish = useCallback(
    (run: AlibabaVoiceRun) => {
      if (!isCurrent(run) || run.status !== "recording" || !run.capture) return;
      run.status = "finishing";
      publishStatus("finishing");
      const capture = run.capture;
      run.capture = null;
      let pcmBase64: string;
      try {
        pcmBase64 = finishVoicePcmCapture(capture);
      } catch (error) {
        failRun(run, error);
        return;
      }
      if (!pcmBase64) {
        failRun(run, "Voice input did not contain audio");
        return;
      }
      run.finishingTimeout = window.setTimeout(() => {
        if (voiceWatchdogAction(run.status, "transcription") === "fail") {
          failRun(run, "Voice transcription timed out");
        }
      }, VOICE_TRANSCRIPTION_TIMEOUT_MS);
      void transcribeVoiceAudio(pcmBase64)
        .then(({ transcript }) => completeRun(run, transcript))
        .catch((error) => failRun(run, error));
    },
    [completeRun, failRun, isCurrent, publishStatus]
  );

  const start = useCallback(async () => {
    if (!enabled || !sessionId || activeRunRef.current) return;
    if (!navigator.mediaDevices?.getUserMedia || typeof AudioContext === "undefined") {
      errorCallbackRef.current("Voice input is unavailable in this runtime");
      return;
    }
    const run: AlibabaVoiceRun = {
      id: ++runSequenceRef.current,
      sessionId,
      status: "connecting",
      capture: null,
      finishingTimeout: null
    };
    activeRunRef.current = run;
    publishStatus("connecting");
    try {
      const capture = await startVoicePcmCapture(() => {
        if (voiceWatchdogAction(run.status, "recording_limit") === "finish") finish(run);
      });
      if (!isCurrent(run)) {
        disposeVoicePcmCapture(capture);
        return;
      }
      run.capture = capture;
      run.status = "recording";
      publishStatus("recording");
    } catch (error) {
      failRun(run, error);
    }
  }, [enabled, failRun, finish, isCurrent, publishStatus, sessionId]);

  const toggle = useCallback(() => {
    const run = activeRunRef.current;
    if (!run) return void start();
    if (run.status === "connecting") forceDispose();
    else if (run.status === "recording") finish(run);
  }, [finish, forceDispose, start]);

  useEffect(() => {
    if (!enabled) forceDispose();
  }, [enabled, forceDispose]);
  useLayoutEffect(() => {
    const run = activeRunRef.current;
    if (run && run.sessionId !== sessionId) forceDispose();
  }, [forceDispose, sessionId]);
  useEffect(() => () => forceDispose(false), [forceDispose]);

  return { status, toggle };
}
