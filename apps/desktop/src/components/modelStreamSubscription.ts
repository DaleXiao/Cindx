import type { ModelStreamDelta } from "../tauriTypes";
import type { ModelStreamAccumulator } from "./modelStreamAccumulator";
import type { ModelStreamEventRouter } from "./modelStreamEventRouter";

export type ModelStreamSubscribe = (
  onDelta: (payload: ModelStreamDelta) => void
) => Promise<() => void>;

type ModelStreamSubscriptionOptions = {
  subscribe: ModelStreamSubscribe;
  router: ModelStreamEventRouter;
  accumulator: ModelStreamAccumulator;
  activeSessionId: () => string | null;
  onStreamDone: (sessionId: string) => boolean | Promise<boolean>;
  onError: (message: string) => void;
};

export function startModelStreamSubscription({
  subscribe,
  router,
  accumulator,
  activeSessionId,
  onStreamDone,
  onError
}: ModelStreamSubscriptionOptions) {
  let disposed = false;
  let unlisten: (() => void) | null = null;

  const finishAndSynchronize = (
    sessionId: string,
    error: string | null,
    allowClear: boolean
  ) => {
    accumulator.finish();
    const terminalSnapshot = accumulator.snapshot();
    const terminalRevision = accumulator.revision;
    if (error) onError(error);
    let synchronized: boolean | Promise<boolean>;
    try {
      synchronized = onStreamDone(sessionId);
    } catch (syncError) {
      onError(syncError instanceof Error ? syncError.message : String(syncError));
      return;
    }
    void Promise.resolve(synchronized)
      .then((durable) => {
        if (
          disposed ||
          !durable ||
          !allowClear ||
          activeSessionId() !== sessionId ||
          accumulator.revision !== terminalRevision ||
          accumulator.snapshot() !== terminalSnapshot
        ) {
          return;
        }
        router.prepareForNextRequest();
        accumulator.reset();
      })
      .catch((syncError) => {
        if (!disposed) onError(syncError instanceof Error ? syncError.message : String(syncError));
      });
  };

  const receive = (payload: ModelStreamDelta) => {
    if (disposed) return;
    const sessionId = activeSessionId();
    const decision = router.route(payload, sessionId);
    if (decision.kind === "ignore") return;

    if (decision.kind === "terminal-refresh") {
      if (sessionId) {
        finishAndSynchronize(sessionId, decision.error, decision.settlesStream);
      }
      return;
    }

    if (decision.reset) accumulator.reset();
    if (decision.delta) accumulator.append(decision.delta);
    if (!decision.done) return;

    if (sessionId) finishAndSynchronize(sessionId, decision.error, true);
  };

  void subscribe(receive)
    .then((handler) => {
      if (disposed) handler();
      else unlisten = handler;
    })
    .catch((error) => {
      if (!disposed) onError(error instanceof Error ? error.message : String(error));
    });

  return () => {
    if (disposed) return;
    disposed = true;
    unlisten?.();
    unlisten = null;
  };
}
