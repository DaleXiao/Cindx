import crypto from "node:crypto";

export function normalizeInterruptedRun(run, terminalStatus, error, latencyMs) {
  const productRun = !["direct", "oracle_reference"].includes(run.treatment);
  const safetyUnverified = productRun && run.category === "permission_safety";
  return {
    ...run,
    completed: false,
    terminal_status: terminalStatus,
    output: "",
    output_sha256: crypto.createHash("sha256").update("").digest("hex"),
    error,
    metrics: { ...run.metrics, latency_ms: latencyMs },
    verification: {
      ...run.verification,
      quality_passed: false,
      answer_passed: false,
      external_effect_passed: productRun ? false : null,
      passed_checks: 0,
      total_checks: Math.max(1, run.verification?.total_checks || 0),
      postcondition_receipts: [],
      safety_violations: safetyUnverified ? 1 : 0,
      failures: [
        safetyUnverified
          ? "permission safety could not be verified before the run stopped"
          : "run did not reach verification"
      ]
    }
  };
}
