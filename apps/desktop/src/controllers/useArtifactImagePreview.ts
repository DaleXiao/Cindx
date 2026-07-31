import { useEffect, useState } from "react";
import { readArtifactImage, readArtifactPreview } from "../tauri";
import { ArtifactImagePreviewCache } from "../utils/artifactImagePreviewCache";

const artifactImagePreviewCache = new ArtifactImagePreviewCache({
  load: async (path) => {
    const [bytes, preview] = await Promise.all([
      readArtifactImage(path),
      readArtifactPreview(path)
    ]);
    if (preview.kind !== "image") {
      throw new Error("artifact is not a supported preview image");
    }
    return { bytes, mimeType: preview.mimeType };
  },
  createUrl: (bytes, mimeType) => {
    const body = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength
    ) as ArrayBuffer;
    return URL.createObjectURL(new Blob([body], { type: mimeType }));
  },
  revokeUrl: (url) => URL.revokeObjectURL(url),
  entryLimit: 8,
  byteLimit: 64 * 1024 * 1024
});

export function useArtifactImagePreview(path: string | null) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let release: (() => void) | null = null;
    setUrl(null);
    if (path) {
      void artifactImagePreviewCache
        .acquire(path)
        .then((handle) => {
          if (active) {
            release = handle.release;
            setUrl(handle.url);
          } else {
            handle.release();
          }
        })
        .catch(() => {
          if (active) setUrl(null);
        });
    }
    return () => {
      active = false;
      release?.();
    };
  }, [path]);

  return url;
}
