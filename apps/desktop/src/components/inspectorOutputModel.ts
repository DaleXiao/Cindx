import type { AgentOutputArtifactView } from "../tauri";

export type OutputArtifact = AgentOutputArtifactView & {
  versionCount: number;
};

export function artifactName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function outputDisplayPath(artifact: OutputArtifact) {
  return artifact.sourcePath ?? artifact.path;
}
