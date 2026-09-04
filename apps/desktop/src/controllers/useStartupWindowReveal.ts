import { useEffect, useRef } from "react";
import { reportFrontendCrash, revealMainWindow } from "../tauri";
import { shouldRevealMainWindow } from "../startupRevealModel";

export type StartupWindowRevealReadiness = {
  runtimeReady: boolean;
  projectSessionReady: boolean;
  bootstrapFailed: boolean;
};

/**
 * Reveals the main window once the initial state is ready, and once — and only
 * once — per app launch.
 *
 * The window stays hidden until then so the first paint is never a half-built
 * shell, and the wait uses a timer rather than `requestAnimationFrame` because
 * timers fire while the window is hidden, so launch can never stall with no UI.
 * A *failed* bootstrap reveals the window too: waiting only for success meant one
 * rejected `get_runtime_status` or `get_project_session_state` left the readiness
 * inputs false forever, so the app ran with no window, no error, and nothing in
 * `startup.log`, and the only recovery was to force-quit it.
 */
export function useStartupWindowReveal(
  readiness: StartupWindowRevealReadiness
): void {
  const revealRequestedRef = useRef(false);
  const { runtimeReady, projectSessionReady, bootstrapFailed } = readiness;

  useEffect(() => {
    if (revealRequestedRef.current) return;
    if (
      !shouldRevealMainWindow({ runtimeReady, projectSessionReady, bootstrapFailed })
    ) {
      return;
    }
    let disposed = false;
    let fontWaitTimer: number | null = null;
    const reveal = async () => {
      try {
        await Promise.race([
          document.fonts.ready,
          new Promise<void>((resolve) => {
            fontWaitTimer = window.setTimeout(resolve, 120);
          })
        ]);
      } catch {}
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
      await new Promise<void>((resolve) => window.setTimeout(resolve, 80));
      if (disposed || revealRequestedRef.current) return;
      revealRequestedRef.current = true;
      await revealMainWindow().catch((error) => {
        void reportFrontendCrash(
          `startup window reveal failed: ${String(error)}`
        );
      });
    };
    void reveal();
    return () => {
      disposed = true;
      if (fontWaitTimer !== null) window.clearTimeout(fontWaitTimer);
    };
  }, [bootstrapFailed, projectSessionReady, runtimeReady]);
}
