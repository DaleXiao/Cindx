import { Component, type ErrorInfo, type ReactNode } from "react";
import { reportFrontendCrash } from "./tauri";

type ErrorBoundaryProps = { children: ReactNode };
type ErrorBoundaryState = { error: Error | null };

/**
 * Top-level render crash net. The window is transparent, so an uncaught
 * render error used to unmount the whole tree and leave a blank window with
 * only the native sidebar material visible. The boundary renders a visible
 * recovery panel instead and reports the stack to the startup log.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const message = `${error.message} :: ${(info.componentStack ?? "unknown component stack").slice(0, 900)}`;
    void reportFrontendCrash(message);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div
        style={{
          display: "grid",
          placeItems: "center",
          height: "100vh",
          padding: 32,
          background: "var(--surface, #ffffff)",
          color: "var(--text, #1f1f1f)",
          fontFamily:
            "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
        }}
      >
        <div style={{ maxWidth: 560, textAlign: "center" }}>
          <h1 style={{ fontSize: 16, fontWeight: 700, margin: "0 0 8px" }}>
            Cindx hit an unexpected UI error
          </h1>
          <p style={{ fontSize: 12.5, lineHeight: 1.5, color: "var(--muted, #707070)" }}>
            Your sessions and data are safe on disk. Reload the window to continue;
            the error detail was recorded in the startup log.
          </p>
          <pre
            style={{
              maxHeight: 180,
              overflow: "auto",
              textAlign: "left",
              fontSize: 11,
              padding: 10,
              borderRadius: 8,
              background: "rgba(0,0,0,0.05)",
              whiteSpace: "pre-wrap",
              overflowWrap: "anywhere"
            }}
          >
            {this.state.error.message}
          </pre>
          <button
            type="button"
            onClick={() => window.location.reload()}
            style={{
              marginTop: 12,
              padding: "8px 18px",
              borderRadius: 999,
              border: "none",
              background: "#2563eb",
              color: "#ffffff",
              fontSize: 12.5,
              fontWeight: 600,
              cursor: "pointer"
            }}
          >
            Reload Cindx
          </button>
        </div>
      </div>
    );
  }
}
