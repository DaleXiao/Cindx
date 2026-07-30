import type { ModelStreamDelta } from "../tauriTypes";

const AGENT_STREAM_TASK_ID = "phase-16-agent-loop";
export const MODEL_STREAM_RETIRED_REQUEST_LIMIT = 8;

export type ModelStreamEventRoute =
  | { kind: "ignore" }
  | { kind: "terminal-refresh"; error: string | null; settlesStream: boolean }
  | {
      kind: "apply";
      requestId: string;
      reset: boolean;
      delta: string;
      done: boolean;
      error: string | null;
    };

export class ModelStreamEventRouter {
  private activeRequestId: string | null = null;
  private resetCandidateId: string | null = null;
  private retiredRequestIds: string[] = [];
  private streamBoundaryPending = false;

  get activeRequest() {
    return this.activeRequestId;
  }

  get retiredRequestCount() {
    return this.retiredRequestIds.length;
  }

  route(payload: ModelStreamDelta, activeSessionId: string | null): ModelStreamEventRoute {
    if (
      payload.taskId !== AGENT_STREAM_TASK_ID ||
      activeSessionId === null ||
      payload.sessionId === null ||
      payload.sessionId !== activeSessionId
    ) {
      return { kind: "ignore" };
    }

    const terminal = payload.done && payload.delta.length === 0;
    if (this.retiredRequestIds.includes(payload.requestId)) {
      return terminal
        ? { kind: "terminal-refresh", error: payload.error, settlesStream: false }
        : { kind: "ignore" };
    }
    if (payload.reset) {
      if (payload.delta.length > 0) this.claim(payload.requestId);
      else this.openResetCandidate(payload.requestId);
      const route = this.apply(payload, true);
      if (payload.done) {
        this.retire(payload.requestId);
        this.streamBoundaryPending = true;
      }
      return route;
    }
    if (terminal && payload.requestId !== this.activeRequestId) {
      this.retire(payload.requestId);
      this.streamBoundaryPending = true;
      return { kind: "terminal-refresh", error: payload.error, settlesStream: true };
    }

    if (this.activeRequestId === null) {
      if (payload.delta.length === 0) return { kind: "ignore" };
      const reset = this.streamBoundaryPending;
      this.claim(payload.requestId);
      const route = this.apply(payload, reset);
      if (payload.done) {
        this.retire(payload.requestId);
        this.streamBoundaryPending = true;
      }
      return route;
    } else if (payload.requestId !== this.activeRequestId) {
      if (this.streamBoundaryPending && payload.delta.length > 0) {
        this.claim(payload.requestId);
        const route = this.apply(payload, true);
        if (payload.done) {
          this.retire(payload.requestId);
          this.streamBoundaryPending = true;
        }
        return route;
      }
      return { kind: "ignore" };
    }

    this.streamBoundaryPending = false;
    const route = this.apply(payload, false);
    if (payload.done) {
      this.retire(payload.requestId);
      this.streamBoundaryPending = true;
    }
    return route;
  }

  reset() {
    this.activeRequestId = null;
    this.resetCandidateId = null;
    this.retiredRequestIds = [];
    this.streamBoundaryPending = false;
  }

  prepareForNextRequest() {
    if (this.activeRequestId) this.remember(this.activeRequestId);
    if (this.resetCandidateId) this.remember(this.resetCandidateId);
    this.activeRequestId = null;
    this.resetCandidateId = null;
    this.streamBoundaryPending = false;
  }

  private apply(payload: ModelStreamDelta, reset: boolean): ModelStreamEventRoute {
    const { requestId, delta, done, error } = payload;
    return { kind: "apply", requestId, reset, delta, done, error };
  }

  private claim(requestId: string) {
    if (this.activeRequestId && this.activeRequestId !== requestId) {
      this.remember(this.activeRequestId);
    }
    if (this.resetCandidateId && this.resetCandidateId !== requestId) {
      this.remember(this.resetCandidateId);
    }
    this.retiredRequestIds = this.retiredRequestIds.filter(
      (retired) => retired !== requestId
    );
    this.resetCandidateId = null;
    this.activeRequestId = requestId;
    this.streamBoundaryPending = false;
  }

  private openResetCandidate(requestId: string) {
    if (this.activeRequestId && this.activeRequestId !== requestId) {
      this.remember(this.activeRequestId);
    }
    if (this.resetCandidateId && this.resetCandidateId !== requestId) {
      this.remember(this.resetCandidateId);
    }
    this.activeRequestId = null;
    this.resetCandidateId = requestId;
    this.streamBoundaryPending = false;
    this.retiredRequestIds = this.retiredRequestIds.filter(
      (retired) => retired !== requestId
    );
  }

  private retire(requestId: string) {
    if (this.activeRequestId === requestId) this.activeRequestId = null;
    if (this.resetCandidateId === requestId) this.resetCandidateId = null;
    this.remember(requestId);
  }

  private remember(requestId: string) {
    if (this.retiredRequestIds.includes(requestId)) return;
    this.retiredRequestIds.push(requestId);
    if (this.retiredRequestIds.length > MODEL_STREAM_RETIRED_REQUEST_LIMIT) {
      this.retiredRequestIds.shift();
    }
  }
}
