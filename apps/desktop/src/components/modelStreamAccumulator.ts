import type { StreamingMarkdownModel, StreamingMarkdownSnapshot } from "./streamingMarkdownModel";

const MEDIUM_STREAM_LENGTH = 16 * 1024;
const LONG_STREAM_LENGTH = 64 * 1024;
const URGENT_PENDING_LENGTH = 4 * 1024;
const PENDING_SLAB_TARGET = 8 * 1024;
const PENDING_PART_LIMIT = 8 * 1024;

export type ModelStreamAccumulatorOptions = {
  model: StreamingMarkdownModel;
  schedule: (callback: () => void, delayMs: number) => number;
  cancel: (handle: number) => void;
  onSnapshot: (snapshot: StreamingMarkdownSnapshot) => void;
};
export class ModelStreamAccumulator {
  private readonly model: StreamingMarkdownModel;
  private readonly schedule: ModelStreamAccumulatorOptions["schedule"];
  private readonly cancel: ModelStreamAccumulatorOptions["cancel"];
  private readonly onSnapshot: (snapshot: StreamingMarkdownSnapshot) => void;
  private pendingSlabs: string[] = [];
  private pendingParts: string[] = [];
  private pendingPartsLength = 0;
  private pendingLength = 0;
  private scheduledHandle: number | null = null;
  private scheduledUrgently = false;
  private generation = 0;
  private visible = true;
  private published = false;
  private hasUnpublishedSnapshot = false;
  private disposed = false;
  private contentRevision = 0;

  constructor({ model, schedule, cancel, onSnapshot }: ModelStreamAccumulatorOptions) {
    this.model = model;
    this.schedule = schedule;
    this.cancel = cancel;
    this.onSnapshot = onSnapshot;
  }

  append(delta: string) {
    if (this.disposed || delta.length === 0) return;
    this.contentRevision += 1;
    if (this.visible && !this.published && this.pendingLength === 0) {
      this.publish(this.model.append(delta));
      return;
    }

    this.enqueue(delta);
    if (!this.visible) {
      if (this.pendingLength >= PENDING_SLAB_TARGET) this.drain(false);
      return;
    }
    if (this.pendingLength >= URGENT_PENDING_LENGTH) {
      if (!this.scheduledUrgently) {
        this.cancelScheduled();
        this.scheduleDrain(16, true);
      }
      return;
    }
    if (this.scheduledHandle === null) this.scheduleDrain(this.adaptiveDelay(), false);
  }

  finish() {
    if (this.disposed) return;
    this.cancelScheduled();
    this.drain(this.visible);
  }

  reset() {
    if (this.disposed) return;
    this.contentRevision += 1;
    this.cancelScheduled();
    this.clearPending();
    this.published = false;
    const snapshot = this.model.reset();
    if (this.visible) this.publish(snapshot);
    else this.hasUnpublishedSnapshot = true;
    this.published = false;
  }

  setVisible(visible: boolean) {
    if (this.disposed || this.visible === visible) return;
    this.visible = visible;
    if (!visible) {
      this.cancelScheduled();
      return;
    }
    this.drain(true);
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.cancelScheduled();
    this.clearPending();
    this.hasUnpublishedSnapshot = false;
  }

  snapshot() {
    return this.model.snapshot();
  }

  get bufferedFragmentCount() {
    return this.pendingSlabs.length + this.pendingParts.length;
  }

  get bufferedCharacterCount() {
    return this.pendingLength;
  }

  get revision() {
    return this.contentRevision;
  }

  private adaptiveDelay() {
    const projectedLength = this.model.snapshot().totalLength + this.pendingLength;
    if (projectedLength >= LONG_STREAM_LENGTH) return 120;
    if (projectedLength >= MEDIUM_STREAM_LENGTH) return 100;
    return 80;
  }

  private scheduleDrain(delayMs: number, urgent: boolean) {
    const generation = this.generation;
    this.scheduledUrgently = urgent;
    this.scheduledHandle = this.schedule(() => {
      if (this.disposed || generation !== this.generation) return;
      this.scheduledHandle = null;
      this.scheduledUrgently = false;
      this.drain();
    }, delayMs);
  }

  private cancelScheduled() {
    this.generation += 1;
    if (this.scheduledHandle !== null) this.cancel(this.scheduledHandle);
    this.scheduledHandle = null;
    this.scheduledUrgently = false;
  }

  private drain(publishSnapshot = true) {
    if (this.pendingLength === 0) {
      if (publishSnapshot && this.hasUnpublishedSnapshot) {
        this.publish(this.model.snapshot());
      }
      return;
    }
    this.flushPendingParts();
    let snapshot = this.model.snapshot();
    for (const slab of this.pendingSlabs) snapshot = this.model.append(slab);
    this.clearPending();
    if (publishSnapshot) this.publish(snapshot);
    else this.hasUnpublishedSnapshot = true;
  }

  private publish(snapshot: StreamingMarkdownSnapshot) {
    this.published = snapshot.totalLength > 0;
    this.hasUnpublishedSnapshot = false;
    this.onSnapshot(snapshot);
  }

  private enqueue(delta: string) {
    this.pendingParts.push(delta);
    this.pendingPartsLength += delta.length;
    this.pendingLength += delta.length;
    if (
      this.pendingPartsLength >= PENDING_SLAB_TARGET ||
      this.pendingParts.length >= PENDING_PART_LIMIT
    ) {
      this.flushPendingParts();
    }
  }

  private flushPendingParts() {
    if (this.pendingPartsLength === 0) return;
    this.pendingSlabs.push(this.pendingParts.join(""));
    this.pendingParts = [];
    this.pendingPartsLength = 0;
  }

  private clearPending() {
    this.pendingSlabs = [];
    this.pendingParts = [];
    this.pendingPartsLength = 0;
    this.pendingLength = 0;
  }
}
