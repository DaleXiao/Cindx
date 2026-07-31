type CacheEntry = {
  url: string;
  bytes: number;
  references: number;
  evicted: boolean;
};

type PendingLoad = {
  promise: Promise<CacheEntry>;
  waiters: number;
};

export type ArtifactImagePreviewHandle = {
  url: string;
  release: () => void;
};

type CacheOptions = {
  load: (path: string) => Promise<{ bytes: Uint8Array; mimeType: string }>;
  createUrl: (bytes: Uint8Array, mimeType: string) => string;
  revokeUrl: (url: string) => void;
  entryLimit: number;
  byteLimit: number;
};

export class ArtifactImagePreviewCache {
  private readonly entries = new Map<string, CacheEntry>();
  private readonly pending = new Map<string, PendingLoad>();
  private readonly options: CacheOptions;
  private retainedBytes = 0;

  constructor(options: CacheOptions) {
    this.options = options;
  }

  acquire(path: string): Promise<ArtifactImagePreviewHandle> {
    const key = path;
    const cached = this.entries.get(key);
    if (cached) {
      this.entries.delete(key);
      this.entries.set(key, cached);
      cached.references += 1;
      return Promise.resolve(this.handle(cached));
    }
    const pending = this.pending.get(key);
    if (pending) {
      pending.waiters += 1;
      return pending.promise.then((entry) => this.handle(entry));
    }

    const request: PendingLoad = { promise: Promise.resolve({} as CacheEntry), waiters: 1 };
    request.promise = this.options.load(path).then(({ bytes, mimeType }) => {
      const url = this.options.createUrl(bytes, mimeType);
      const entry = {
        url,
        bytes: bytes.byteLength,
        references: request.waiters,
        evicted: false
      };
      this.insert(key, entry);
      return entry;
    });
    this.pending.set(key, request);
    void request.promise.finally(() => this.pending.delete(key)).catch(() => {});
    return request.promise.then((entry) => this.handle(entry));
  }

  private insert(key: string, entry: CacheEntry) {
    if (entry.bytes > this.options.byteLimit || this.options.entryLimit === 0) {
      this.options.revokeUrl(entry.url);
      throw new Error("artifact image exceeds the preview cache limit");
    }
    while (
      this.entries.size >= this.options.entryLimit ||
      this.retainedBytes + entry.bytes > this.options.byteLimit
    ) {
      const oldestKey = this.entries.keys().next().value;
      if (!oldestKey) break;
      const oldest = this.entries.get(oldestKey);
      this.entries.delete(oldestKey);
      if (oldest) {
        this.retainedBytes -= oldest.bytes;
        oldest.evicted = true;
        if (oldest.references === 0) this.options.revokeUrl(oldest.url);
      }
    }
    this.entries.set(key, entry);
    this.retainedBytes += entry.bytes;
  }

  private handle(entry: CacheEntry): ArtifactImagePreviewHandle {
    let released = false;
    return {
      url: entry.url,
      release: () => {
        if (released) return;
        released = true;
        entry.references = Math.max(0, entry.references - 1);
        if (entry.evicted && entry.references === 0) {
          this.options.revokeUrl(entry.url);
        }
      }
    };
  }
}
