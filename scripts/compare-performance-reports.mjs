import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);

function option(name, fallback = null) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}

function requiredPath(name) {
  const value = option(name);
  if (!value) throw new Error(`Missing required option ${name}`);
  return path.resolve(value);
}

function positiveNumber(name, fallback) {
  const value = Number(option(name, String(fallback)));
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`${name} must be a non-negative number`);
  }
  return value;
}

function loadReport(reportPath) {
  const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));
  if (!Array.isArray(report.diagnostics)) {
    throw new Error(`${reportPath} does not contain structured diagnostics`);
  }
  return report;
}

function loadPolicy(policyPath) {
  if (!policyPath) return null;
  const policy = JSON.parse(fs.readFileSync(policyPath, "utf8"));
  if (
    policy.schema !== "cindx.performance-policy.v1" ||
    !Array.isArray(policy.workloads)
  ) {
    throw new Error(`${policyPath} is not a Cindx performance policy`);
  }
  return policy;
}

function diagnosticMap(report) {
  return new Map(report.diagnostics.map((diagnostic) => [diagnostic.schema, diagnostic]));
}

const defaultWorkloads = [
  {
    schema: "cindx.session-projection-diagnostic.v1",
    identity: ["initial_events", "delta_events_read", "warm_sample_count"],
    metrics: ["warm_p95_micros"]
  },
  {
    schema: "cindx.context-governor-diagnostic.v1",
    identity: ["history_messages", "sample_count"],
    metrics: ["p95_micros"]
  },
  {
    schema: "cindx.rag-search-diagnostic.v1",
    identity: ["chunks", "dimensions", "sample_count"],
    metrics: ["semantic_p95_micros", "literal_p95_micros"]
  }
];

const baselinePath = requiredPath("--baseline");
const candidatePath = requiredPath("--candidate");
const outputPath = option("--report");
const maxRegressionPercent = positiveNumber("--max-regression-percent", 25);
const absoluteToleranceMicros = positiveNumber("--absolute-tolerance-micros", 1_000);
const policyPath = option("--policy");
const policy = loadPolicy(policyPath ? path.resolve(policyPath) : null);
const workloads = policy?.workloads ?? defaultWorkloads;
const baseline = diagnosticMap(loadReport(baselinePath));
const candidate = diagnosticMap(loadReport(candidatePath));
const comparisons = [];

for (const workload of workloads) {
  const before = baseline.get(workload.schema);
  const after = candidate.get(workload.schema);
  if (!before || !after) {
    comparisons.push({
      schema: workload.schema,
      passed: false,
      errors: [`missing ${!before ? "baseline" : "candidate"} diagnostic`]
    });
    continue;
  }
  const identityErrors = workload.identity.flatMap((field) =>
    Object.is(before[field], after[field])
      ? []
      : [`${field} differs: baseline=${before[field]} candidate=${after[field]}`]
  );
  const metrics = workload.metrics.map((metric) => {
    const field = typeof metric === "string" ? metric : metric.field;
    const metricMaxRegressionPercent =
      typeof metric === "string"
        ? maxRegressionPercent
        : metric.max_regression_percent ?? maxRegressionPercent;
    const metricAbsoluteToleranceMicros =
      typeof metric === "string"
        ? absoluteToleranceMicros
        : metric.absolute_tolerance_micros ?? absoluteToleranceMicros;
    const baselineValue = Number(before[field]);
    const candidateValue = Number(after[field]);
    const allowed = Math.max(
      baselineValue * (1 + metricMaxRegressionPercent / 100),
      baselineValue + metricAbsoluteToleranceMicros
    );
    return {
      field,
      baseline: baselineValue,
      candidate: candidateValue,
      allowed,
      max_regression_percent: metricMaxRegressionPercent,
      absolute_tolerance_micros: metricAbsoluteToleranceMicros,
      passed:
        Number.isFinite(baselineValue) &&
        Number.isFinite(candidateValue) &&
        candidateValue <= allowed
    };
  });
  comparisons.push({
    schema: workload.schema,
    passed: identityErrors.length === 0 && metrics.every((metric) => metric.passed),
    errors: identityErrors,
    metrics
  });
}

const result = {
  schema: "cindx.performance-comparison.v1",
  baseline: baselinePath,
  candidate: candidatePath,
  policy: policyPath ? path.resolve(policyPath) : null,
  max_regression_percent: maxRegressionPercent,
  absolute_tolerance_micros: absoluteToleranceMicros,
  passed: comparisons.every((comparison) => comparison.passed),
  comparisons
};
const serialized = `${JSON.stringify(result, null, 2)}\n`;
if (outputPath) {
  const absoluteOutputPath = path.resolve(outputPath);
  fs.mkdirSync(path.dirname(absoluteOutputPath), { recursive: true });
  fs.writeFileSync(absoluteOutputPath, serialized);
}
process.stdout.write(serialized);
if (!result.passed) process.exitCode = 1;
