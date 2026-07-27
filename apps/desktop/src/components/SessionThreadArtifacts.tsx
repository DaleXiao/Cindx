import { FileText, FolderOpen, Image as ImageIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { openArtifact, readArtifactPreview } from "../tauri";
import type { AgentAttachment, AgentOutputArtifactView } from "../tauri";

const MESSAGE_ATTACHMENT_PREVIEW_CACHE_LIMIT = 8;
const messageAttachmentPreviewCache = new Map<string, string>();

function cacheMessageAttachmentPreview(path: string, dataUrl: string) {
  messageAttachmentPreviewCache.delete(path);
  messageAttachmentPreviewCache.set(path, dataUrl);
  while (messageAttachmentPreviewCache.size > MESSAGE_ATTACHMENT_PREVIEW_CACHE_LIMIT) {
    const oldestPath = messageAttachmentPreviewCache.keys().next().value;
    if (!oldestPath) break;
    messageAttachmentPreviewCache.delete(oldestPath);
  }
}

function MessageAttachmentPreview({ attachment }: { attachment: AgentAttachment }) {
  const [dataUrl, setDataUrl] = useState<string | null>(
    () => messageAttachmentPreviewCache.get(attachment.path) ?? null
  );

  useEffect(() => {
    const cached = messageAttachmentPreviewCache.get(attachment.path);
    if (cached) {
      setDataUrl(cached);
      return;
    }
    let active = true;
    setDataUrl(null);
    void readArtifactPreview(attachment.path)
      .then((preview) => {
        if (!active || preview.kind !== "image" || !preview.dataUrl) return;
        cacheMessageAttachmentPreview(attachment.path, preview.dataUrl);
        setDataUrl(preview.dataUrl);
      })
      .catch(() => {
        if (active) setDataUrl(null);
      });
    return () => {
      active = false;
    };
  }, [attachment.path]);

  if (dataUrl) {
    return <img src={dataUrl} alt={attachment.name} />;
  }
  return (
    <span className="thread-message-attachment-placeholder">
      <ImageIcon aria-hidden="true" />
      <span>{attachment.name}</span>
    </span>
  );
}

export function UserMessageAttachments({
  attachments,
  onOpenError
}: {
  attachments: AgentAttachment[];
  onOpenError: (message: string) => void;
}) {
  return (
    <div className="thread-message-attachments" aria-label="Message attachments">
      {attachments.map((attachment) => {
        const isImage = attachment.mimeType.startsWith("image/");
        return (
          <button
            className="thread-message-attachment"
            data-image={isImage}
            type="button"
            aria-label={`Open ${attachment.name}`}
            title={attachment.name}
            key={attachment.id}
            onClick={(event) => {
              event.stopPropagation();
              void openArtifact(attachment.path).catch((error) => {
                const detail = error instanceof Error ? error.message : String(error);
                onOpenError(`Could not open ${attachment.name}: ${detail}`);
              });
            }}
          >
            {isImage ? (
              <MessageAttachmentPreview attachment={attachment} />
            ) : (
              <>
                <FileText aria-hidden="true" />
                <span>{attachment.name}</span>
              </>
            )}
          </button>
        );
      })}
    </div>
  );
}

function artifactName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function artifactDisplayPath(artifact: AgentOutputArtifactView) {
  return artifact.sourcePath ?? artifact.path;
}

function ArtifactImagePreview({ artifact }: { artifact: AgentOutputArtifactView }) {
  const [dataUrl, setDataUrl] = useState<string | null>(
    () => messageAttachmentPreviewCache.get(artifact.path) ?? null
  );

  useEffect(() => {
    const cached = messageAttachmentPreviewCache.get(artifact.path);
    if (cached) {
      setDataUrl(cached);
      return;
    }
    let active = true;
    setDataUrl(null);
    void readArtifactPreview(artifact.path)
      .then((preview) => {
        if (!active || preview.kind !== "image" || !preview.dataUrl) return;
        cacheMessageAttachmentPreview(artifact.path, preview.dataUrl);
        setDataUrl(preview.dataUrl);
      })
      .catch(() => {
        if (active) setDataUrl(null);
      });
    return () => {
      active = false;
    };
  }, [artifact.path]);

  if (dataUrl) return <img src={dataUrl} alt={artifactName(artifactDisplayPath(artifact))} />;
  return (
    <span className="thread-output-image-placeholder">
      <ImageIcon aria-hidden="true" />
    </span>
  );
}

export function ThreadOutputArtifacts({
  artifacts,
  onInspect,
  onOpenError
}: {
  artifacts: AgentOutputArtifactView[];
  onInspect: (path: string) => void;
  onOpenError: (message: string) => void;
}) {
  if (artifacts.length === 0) return null;

  const openWithSystem = (artifact: AgentOutputArtifactView) => {
    void openArtifact(artifact.path).catch((error) => {
      const detail = error instanceof Error ? error.message : String(error);
      onOpenError(`Could not open ${artifactName(artifactDisplayPath(artifact))}: ${detail}`);
    });
  };

  return (
    <div
      className="thread-output-artifacts"
      aria-label="Agent outputs"
      onClick={(event) => event.stopPropagation()}
    >
      {artifacts.map((artifact) => {
        const displayPath = artifactDisplayPath(artifact);
        const name = artifactName(displayPath);
        const versionLabel = artifact.version > 1 ? `v${artifact.version}` : null;
        if (artifact.kind === "image") {
          return (
            <div className="thread-output-image" key={`${artifact.id}-${artifact.path}`}>
              <button
                className="thread-output-image-preview"
                type="button"
                aria-label={`Open ${name}`}
                title={`Open ${name}`}
                onClick={() => openWithSystem(artifact)}
              >
                <ArtifactImagePreview artifact={artifact} />
              </button>
              <button
                className="thread-output-name"
                type="button"
                title={`Preview ${name}`}
                onClick={() => onInspect(artifact.path)}
              >
                <span>{name}</span>
                {versionLabel && <small>{versionLabel}</small>}
              </button>
            </div>
          );
        }

        return (
          <button
            className="thread-output-link"
            type="button"
            key={`${artifact.id}-${artifact.path}`}
            title={artifact.kind === "directory" ? `Open ${displayPath} in Finder` : `Preview ${name}`}
            onClick={() => {
              if (artifact.kind === "directory") openWithSystem(artifact);
              else onInspect(artifact.path);
            }}
          >
            {artifact.kind === "directory" ? (
              <FolderOpen aria-hidden="true" />
            ) : (
              <FileText aria-hidden="true" />
            )}
            <span>{artifact.kind === "directory" ? displayPath : name}</span>
            {versionLabel && <small>{versionLabel}</small>}
          </button>
        );
      })}
    </div>
  );
}
