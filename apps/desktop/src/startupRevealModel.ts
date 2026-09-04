/**
 * When the main window may reveal itself at startup.
 *
 * The window stays hidden until the initial state is ready so the first paint is
 * never a half-built shell. Waiting only for *success*, though, meant that one
 * rejected `get_runtime_status` or `get_project_session_state` left `runtime` and
 * `projectSessionState` null forever: the reveal effect returned early on every
 * render, so the process ran with no window, no error, and no log entry, and the
 * only recovery was to force-quit it. A failed bootstrap therefore reveals the
 * window as well — an empty workspace the user can see, report, and retry from
 * beats an invisible process.
 */
export type StartupRevealReadiness = {
  runtimeReady: boolean;
  projectSessionReady: boolean;
  bootstrapFailed: boolean;
};

export function shouldRevealMainWindow(
  readiness: StartupRevealReadiness
): boolean {
  if (readiness.runtimeReady && readiness.projectSessionReady) {
    return true;
  }
  // Both requests settled and at least one failed: waiting longer cannot help.
  return readiness.bootstrapFailed;
}
