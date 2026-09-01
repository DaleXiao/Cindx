export type FailurePresentation = {
  /** Plain-language summary shown to the user. */
  summary: string;
  /** The raw internal message, shown behind a details toggle when it differs. */
  detail: string | null;
};

/**
 * Translate stable internal failure messages into plain language a user can
 * act on. Contract/repair failures are the agent's own bookkeeping; surfacing
 * them verbatim reads as an inscrutable exception. The raw text is retained as
 * `detail` for diagnostics. Unknown messages pass through unchanged so a new
 * failure is never silently hidden.
 */
export function presentFailure(raw: string): FailurePresentation {
  const trimmed = raw.trim();

  const unsatisfied = trimmed.match(
    /^task contract requirement `([^`]+)` remained unsatisfied after (\d+) repair attempts?$/
  );
  if (unsatisfied) {
    const tool = unsatisfied[1].split(":").pop() ?? unsatisfied[1];
    return {
      summary: `A planned step didn't finish: ${tool}. This usually means you cancelled or redirected it mid-run. Dismiss this if you no longer need that step, or retry to let the agent complete it.`,
      detail: trimmed,
    };
  }

  const unavailable = trimmed.match(/^task contract requires unavailable tool `([^`]+)`$/);
  if (unavailable) {
    return {
      summary: `The current plan needs a tool that isn't available in this run: ${unavailable[1]}. Retry with a different approach, or dismiss.`,
      detail: trimmed,
    };
  }

  return { summary: trimmed, detail: null };
}
