function percent(value) {
  return value === null ? "n/a" : `${(value * 100).toFixed(1)}%`;
}

function signed(value, suffix = "") {
  if (value === null) return "n/a";
  return `${value >= 0 ? "+" : ""}${value}${suffix}`;
}

export function renderCurrentRealworldMarkdown(report) {
  const lines = [
    `# Cindx Agent Real-World ${report.decision.status === "VALID_BASELINE" ? "Baseline" : "Evaluation"} ${report.evidence.app_version}`,
    "",
    "## Evidence",
    "",
    `- Status: **${report.decision.status}**`,
    `- Application version: \`${report.evidence.app_version}\``,
    `- Git commit: \`${report.evidence.git_commit}\``,
    `- Frozen suite: \`${report.suite.id}@${report.suite.version}\` (\`${report.suite.sha256}\`)`,
    `- Matrix: ${report.suite.cases.length} cases × ${report.suite.treatments.length} treatments × ${report.suite.replicates} replicates`,
    `- Execution plan: \`${report.evidence.execution_plan_sha256}\``,
    `- Raw evidence SHA-256: \`${report.evidence.raw_sha256}\``,
    `- Missing or structurally unverifiable cells: ${report.evidence.incomplete_runs}`,
    `- Provider-evidence incomplete runs: ${report.evidence.provider_evidence_incomplete_runs}`,
    `- Strategy-evidence incomplete runs: ${report.evidence.strategy_evidence_incomplete_runs}`,
    `- Non-completed runs retained in the denominator: ${report.outcomes.non_completed_runs}`,
    "",
    "## Treatment Contract",
    "",
    "- `oracle_reference` is a no-tools reference ceiling and is not a product baseline.",
    "- `grounded_direct` runs the shipping AgentKernel with Auto's budget, model routing, tools, retrieval, memory, permissions, and external postcondition verifier, but clamps collaboration to direct execution. Independent worker verification is therefore normalized to self-check.",
    "- `auto` is the iso-budget adaptive candidate. `pro` remains descriptive because its native budget differs.",
    "",
    "| Treatment | Complete | Quality | External effect | Safety | Median latency | Tokens | Model calls | Tool calls |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
  ];
  for (const treatment of report.suite.treatments) {
    const result = report.aggregates[treatment];
    lines.push(
      `| ${treatment} | ${percent(result.completion_rate)} | ${percent(result.quality_pass_rate)} | ${percent(result.external_effect_pass_rate)} | ${result.safety_violations} | ${result.latency_ms.median} ms | ${result.total_tokens} | ${result.model_calls} | ${result.tool_calls} |`
    );
  }
  lines.push(
    "",
    "## Mechanism Claims",
    "",
    "| Dimension | State | Evidence complete | Pairs / denominator | Quality delta runs | Completion delta runs | Reason |",
    "| --- | --- | --- | ---: | ---: | ---: | --- |"
  );
  for (const dimension of [
    "adaptive_direct",
    "workflow",
    "learned_profile",
    "distillation"
  ]) {
    const claim = report.decision.mechanism_claims[dimension];
    lines.push(
      `| ${dimension} | ${claim.state} | ${claim.evidence_complete ? "yes" : "no"} | ${claim.matched_pairs} / ${claim.denominator} | ${signed(claim.quality_delta_runs)} | ${signed(claim.completion_delta_runs)} | ${claim.reason} |`
    );
  }
  lines.push(
    "",
    "## Paired Against Grounded Direct",
    "",
    "| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |",
    "| --- | ---: | ---: | ---: | ---: |"
  );
  for (const treatment of ["auto", "pro"]) {
    const paired = report.paired_against_baseline[treatment];
    lines.push(
      `| ${treatment} | ${paired.pairs} | ${paired.quality_pass_delta === null ? "n/a" : signed((paired.quality_pass_delta * 100).toFixed(1), " pp")} | ${paired.completion_delta === null ? "n/a" : signed((paired.completion_delta * 100).toFixed(1), " pp")} | ${signed(paired.median_latency_delta_ms, " ms")} |`
    );
  }
  lines.push(
    "",
    "## Interpretation Boundary",
    "",
    report.decision.claim_boundary,
    "Failures, timeouts, and denials remain in the denominator. Deterministic checks validate the mechanism only; intelligence conclusions require this provider-backed paired evidence.",
    "Raw prompts and model outputs remain outside Git."
  );
  return `${lines.join("\n")}\n`;
}
