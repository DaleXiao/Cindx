import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { subscribeToModelStream } from "../tauri";
import { ModelStreamAccumulator } from "./modelStreamAccumulator";
import { ModelStreamEventRouter } from "./modelStreamEventRouter";
import { startModelStreamSubscription } from "./modelStreamSubscription";
import {
  StreamingMarkdownModel,
  type StreamingMarkdownSnapshot
} from "./streamingMarkdownModel";

type UseModelStreamAnswerOptions = {
  sessionId: string | null;
  streamResetVersion: number;
  onStreamDone: (sessionId: string) => boolean | Promise<boolean>;
  onError: (message: string) => void;
};

type StreamSnapshotState = {
  sessionId: string | null;
  resetVersion: number;
  snapshot: StreamingMarkdownSnapshot | null;
};

export function useModelStreamAnswer({
  sessionId,
  streamResetVersion,
  onStreamDone,
  onError
}: UseModelStreamAnswerOptions): StreamingMarkdownSnapshot | null {
  const sessionIdRef = useRef(sessionId);
  const resetVersionRef = useRef(streamResetVersion);
  const onStreamDoneRef = useRef(onStreamDone);
  const onErrorRef = useRef(onError);
  const eventRouterRef = useRef(new ModelStreamEventRouter());
  const accumulatorRef = useRef<ModelStreamAccumulator | null>(null);
  const lifecycleGenerationRef = useRef(0);
  const previousSessionIdRef = useRef(sessionId);
  const [streamState, setStreamState] = useState<StreamSnapshotState>({
    sessionId,
    resetVersion: streamResetVersion,
    snapshot: null
  });

  sessionIdRef.current = sessionId;
  resetVersionRef.current = streamResetVersion;
  onStreamDoneRef.current = onStreamDone;
  onErrorRef.current = onError;

  useLayoutEffect(() => {
    if (previousSessionIdRef.current === sessionId) {
      eventRouterRef.current.prepareForNextRequest();
    } else {
      eventRouterRef.current.reset();
    }
    previousSessionIdRef.current = sessionId;
    accumulatorRef.current?.reset();
    setStreamState({
      sessionId,
      resetVersion: streamResetVersion,
      snapshot: null
    });
  }, [sessionId, streamResetVersion]);

  useEffect(() => {
    const generation = lifecycleGenerationRef.current + 1;
    lifecycleGenerationRef.current = generation;
    let disposed = false;

    const accumulator = new ModelStreamAccumulator({
      model: new StreamingMarkdownModel(),
      schedule: (callback, delayMs) => window.setTimeout(callback, delayMs),
      cancel: (timerId) => window.clearTimeout(timerId),
      onSnapshot: (snapshot) => {
        if (disposed || lifecycleGenerationRef.current !== generation) return;
        setStreamState({
          sessionId: sessionIdRef.current,
          resetVersion: resetVersionRef.current,
          snapshot: snapshot.totalLength > 0 ? snapshot : null
        });
      }
    });
    accumulatorRef.current = accumulator;

    const syncVisibility = () => {
      accumulator.setVisible(document.visibilityState !== "hidden");
    };
    syncVisibility();
    document.addEventListener("visibilitychange", syncVisibility);

    const stopSubscription = startModelStreamSubscription({
      subscribe: subscribeToModelStream,
      router: eventRouterRef.current,
      accumulator,
      activeSessionId: () => sessionIdRef.current,
      onStreamDone: (activeSessionId) => onStreamDoneRef.current(activeSessionId),
      onError: (message) => onErrorRef.current(message)
    });

    return () => {
      disposed = true;
      lifecycleGenerationRef.current += 1;
      stopSubscription();
      document.removeEventListener("visibilitychange", syncVisibility);
      accumulator.dispose();
      eventRouterRef.current.reset();
      if (accumulatorRef.current === accumulator) accumulatorRef.current = null;
    };
  }, []);

  if (
    streamState.sessionId !== sessionId ||
    streamState.resetVersion !== streamResetVersion
  ) {
    return null;
  }
  return streamState.snapshot;
}
