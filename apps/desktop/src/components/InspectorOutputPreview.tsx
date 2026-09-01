import { ExternalLink, Maximize2, Minimize2, X } from "lucide-react";
import Markdown from "markdown-to-jsx";
import { useEffect, useState } from "react";
import { useArtifactImagePreview } from "../controllers/useArtifactImagePreview";
import { readArtifactPreview } from "../tauri";
import type { ArtifactPreview } from "../tauri";
import { useDialogFocus } from "../useDialogFocus";
import { artifactName, outputDisplayPath, type OutputArtifact } from "./inspectorOutputModel";

type InspectorOutputPreviewProps = {
  output: OutputArtifact;
  fullscreen: boolean;
  openingPath: string | null;
  actionError: string | null;
  onToggleFullscreen: () => void;
  onOpen: () => void;
  onClose: () => void;
};

/**
 * The output preview panel (inline or fullscreen portal body). In fullscreen
 * it is a modal dialog: initial focus, Tab wrap, Escape-to-exit, and focus
 * restore come from the shared dialog focus controller.
 */
export function InspectorOutputPreview({
  output,
  fullscreen,
  openingPath,
  actionError,
  onToggleFullscreen,
  onOpen,
  onClose
}: InspectorOutputPreviewProps) {
  const dialogRef = useDialogFocus<HTMLElement>(fullscreen, {
    onEscape: onToggleFullscreen
  });
  return (
    <section
      ref={fullscreen ? dialogRef : undefined}
      className="inspector-output-detail"
      aria-label="Output preview"
      aria-modal={fullscreen || undefined}
      data-fullscreen={fullscreen}
      role={fullscreen ? "dialog" : undefined}
    >
      <header title={outputDisplayPath(output)}>
        <div className="inspector-output-detail-copy">
          <strong>{artifactName(outputDisplayPath(output))}</strong>
          <span>
            {output.toolName}
            {output.versionCount > 1 ? ` · Version ${output.version}` : ""}
          </span>
        </div>
        <div className="inspector-output-actions">
          <button
            className="icon-button quiet"
            type="button"
            aria-label={fullscreen ? "Exit full screen" : "Show full screen"}
            title={fullscreen ? "Exit full screen" : "Show full screen"}
            onClick={onToggleFullscreen}
          >
            {fullscreen ? <Minimize2 aria-hidden="true" /> : <Maximize2 aria-hidden="true" />}
          </button>
          <button
            className="icon-button quiet"
            type="button"
            aria-label="Open with default app"
            title="Open with default app"
            disabled={openingPath === output.path}
            onClick={onOpen}
          >
            <ExternalLink aria-hidden="true" />
          </button>
          <button
            className="icon-button quiet"
            type="button"
            aria-label="Close preview"
            title="Close preview"
            onClick={onClose}
          >
            <X aria-hidden="true" />
          </button>
        </div>
      </header>
      <div className="inspector-output-action-error" role="alert">
        {actionError}
      </div>
      <div className="inspector-output-detail-body">
        <ArtifactPreviewPane path={output.path} />
      </div>
    </section>
  );
}

function ArtifactPreviewPane({ path }: { path: string }) {
  const [preview, setPreview] = useState<ArtifactPreview | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setError(null);
    void readArtifactPreview(path)
      .then((next) => {
        if (active) setPreview(next);
      })
      .catch((reason) => {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      active = false;
    };
  }, [path]);

  if (error) return <div className="inspector-preview-message">{error}</div>;
  if (!preview) return <div className="inspector-preview-message">Loading preview</div>;
  if (preview.kind === "image") {
    return <ArtifactImagePreviewPane path={path} />;
  }
  if (preview.kind === "html" && preview.content != null) {
    return (
      <iframe
        className="inspector-preview-frame"
        title={`Preview ${artifactName(path)}`}
        sandbox=""
        srcDoc={preview.content}
      />
    );
  }
  if (preview.kind === "markdown" && preview.content != null) {
    return (
      <Markdown className="thread-markdown inspector-markdown-preview">{preview.content}</Markdown>
    );
  }
  if (preview.kind === "text" && preview.content != null) {
    return <pre className="inspector-text-preview">{preview.content}</pre>;
  }
  return <div className="inspector-preview-message">Preview unavailable for this file type</div>;
}

function ArtifactImagePreviewPane({ path }: { path: string }) {
  const url = useArtifactImagePreview(path);
  if (!url) return <div className="inspector-preview-message">Loading preview</div>;
  return <img className="inspector-preview-image" src={url} alt={artifactName(path)} />;
}
