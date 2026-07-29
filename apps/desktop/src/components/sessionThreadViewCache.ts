import type { AgentOutputArtifactView } from "../tauri";

export type EvictedThreadRowHeight = {
  sessionId: string;
  rowKey: string;
};

function artifactsEqual(
  current: AgentOutputArtifactView[],
  incoming: AgentOutputArtifactView[]
) {
  return (
    current.length === incoming.length &&
    current.every((artifact, index) => {
      const next = incoming[index];
      return (
        artifact.id === next?.id &&
        artifact.path === next.path &&
        artifact.sourcePath === next.sourcePath &&
        artifact.toolName === next.toolName &&
        artifact.status === next.status &&
        artifact.timestampMs === next.timestampMs &&
        artifact.runId === next.runId &&
        artifact.version === next.version &&
        artifact.kind === next.kind
      );
    })
  );
}

export class SessionThreadViewCache {
  private readonly rowHeights = new Map<string, Map<string, number>>();
  private readonly artifacts = new Map<string, AgentOutputArtifactView[]>();
  private readonly artifactLoads = new Map<
    string,
    Promise<AgentOutputArtifactView[]>
  >();
  private readonly artifactReloads = new Set<string>();
  private readonly sessionLimit: number;
  private readonly rowLimit: number;
  private viewportWidth: number | null = null;

  constructor(sessionLimit: number, rowLimit: number) {
    this.sessionLimit = sessionLimit;
    this.rowLimit = rowLimit;
  }

  rowHeight(sessionId: string, rowKey: string) {
    return this.rowHeights.get(sessionId)?.get(rowKey);
  }

  updateViewportWidth(width: number) {
    const nextWidth = Math.round(width);
    if (this.viewportWidth === null) {
      this.viewportWidth = nextWidth;
      return false;
    }
    if (this.viewportWidth === nextWidth) return false;
    this.viewportWidth = nextWidth;
    this.rowHeights.clear();
    return true;
  }

  rememberRowHeight(sessionId: string, rowKey: string, height: number) {
    const evicted: EvictedThreadRowHeight[] = [];
    let sessionRows = this.rowHeights.get(sessionId);
    if (!sessionRows) {
      sessionRows = new Map();
    } else {
      this.rowHeights.delete(sessionId);
    }
    this.rowHeights.set(sessionId, sessionRows);
    sessionRows.delete(rowKey);
    sessionRows.set(rowKey, height);

    while (sessionRows.size > this.rowLimit) {
      const oldestRowKey = sessionRows.keys().next().value;
      if (oldestRowKey === undefined) break;
      sessionRows.delete(oldestRowKey);
      evicted.push({ sessionId, rowKey: oldestRowKey });
    }
    while (this.rowHeights.size > this.sessionLimit) {
      const oldestSessionId = this.rowHeights.keys().next().value;
      if (oldestSessionId === undefined) break;
      const oldestRows = this.rowHeights.get(oldestSessionId);
      this.rowHeights.delete(oldestSessionId);
      oldestRows?.forEach((_, oldestRowKey) => {
        evicted.push({ sessionId: oldestSessionId, rowKey: oldestRowKey });
      });
    }
    return evicted;
  }

  artifactsFor(sessionId: string) {
    return this.artifacts.get(sessionId) ?? null;
  }

  loadArtifacts(
    sessionId: string,
    load: () => Promise<AgentOutputArtifactView[]>
  ) {
    const pending = this.artifactLoads.get(sessionId);
    if (pending) {
      this.artifactReloads.add(sessionId);
      return pending;
    }
    const run = async () => {
      let artifacts = await load();
      while (this.artifactReloads.delete(sessionId)) artifacts = await load();
      return this.rememberArtifacts(sessionId, artifacts);
    };
    const request = run();
    this.artifactLoads.set(sessionId, request);
    const clear = () => {
      if (this.artifactLoads.get(sessionId) === request) {
        this.artifactLoads.delete(sessionId);
        this.artifactReloads.delete(sessionId);
      }
    };
    void request.then(clear, clear);
    return request;
  }

  rememberArtifacts(sessionId: string, artifacts: AgentOutputArtifactView[]) {
    const current = this.artifacts.get(sessionId);
    const stable = current && artifactsEqual(current, artifacts) ? current : artifacts;
    this.artifacts.delete(sessionId);
    this.artifacts.set(sessionId, stable);
    while (this.artifacts.size > this.sessionLimit) {
      const oldestSessionId = this.artifacts.keys().next().value;
      if (oldestSessionId === undefined) break;
      this.artifacts.delete(oldestSessionId);
    }
    return stable;
  }
}
