import crypto from "node:crypto";

const sha256Pattern = /^[0-9a-f]{64}$/;
const isOracleTreatment = (treatment) =>
  treatment === "direct" || treatment === "oracle_reference";
const postconditionSubjectDomain = Buffer.from(
  "cindx.agent-realworld-postcondition-subject.v1\0"
);
const toolReceiptStatuses = [
  "succeeded",
  "failed",
  "cancelled",
  "denied",
  "incomplete",
  "superseded",
  "invalid"
];

export const toolReceiptMetricFields = {
  succeeded: "tool_succeeded",
  failed: "tool_failed",
  cancelled: "tool_cancelled",
  denied: "tool_denied",
  incomplete: "tool_incomplete",
  superseded: "tool_superseded",
  invalid: "tool_invalid"
};

function requireFact(condition, message) {
  if (!condition) throw new Error(message);
}

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function nonNegativeInteger(value, label) {
  requireFact(Number.isInteger(value) && value >= 0, `${label} must be a non-negative integer`);
  return value;
}

function exactFields(value, fields, label) {
  requireFact(value && typeof value === "object" && !Array.isArray(value), `${label} is invalid`);
  requireFact(
    JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...fields].sort()),
    `${label} fields drifted`
  );
}

function nullableSha256(value, label) {
  requireFact(value === null || sha256Pattern.test(value), `${label} is invalid`);
}

export function validateSuiteReceiptContracts(testCase) {
  const verification = testCase.verification;
  requireFact(verification && typeof verification === "object", `${testCase.id}: verification is missing`);
  for (const field of ["required_tools_any", "required_tools_all"]) {
    requireFact(Array.isArray(verification[field]), `${testCase.id}: ${field} is missing`);
    requireFact(
      verification[field].every((tool) => typeof tool === "string" && tool.length > 0) &&
        new Set(verification[field]).size === verification[field].length,
      `${testCase.id}: ${field} is invalid`
    );
  }
  const browserReceipt = verification.browser_target_receipt;
  if (testCase.category === "browser") {
    exactFields(
      browserReceipt,
      ["tools_all", "tools_any", "minimum_artifacts"],
      `${testCase.id}: browser target receipt`
    );
    for (const field of ["tools_all", "tools_any"]) {
      requireFact(
        Array.isArray(browserReceipt[field]) &&
          browserReceipt[field].length > 0 &&
          browserReceipt[field].every((tool) => typeof tool === "string" && tool.length > 0),
        `${testCase.id}: browser target ${field} is invalid`
      );
    }
    requireFact(
      browserReceipt.tools_all.every((tool) => verification.required_tools_all.includes(tool)) &&
        browserReceipt.tools_any.every((tool) => verification.required_tools_any.includes(tool)),
      `${testCase.id}: browser target tools must be declared as required tools`
    );
    requireFact(
      Number.isInteger(browserReceipt.minimum_artifacts) && browserReceipt.minimum_artifacts > 0,
      `${testCase.id}: browser minimum artifacts is invalid`
    );
    requireFact(
      testCase.files?.some(
        (fixture) => fixture.path === "site/index.html" && typeof fixture.content === "string"
      ),
      `${testCase.id}: HTTP fixture body is missing`
    );
  } else {
    requireFact(browserReceipt === undefined, `${testCase.id}: non-browser case claims a browser receipt`);
  }
}

function toolRequirementSatisfied(observed, required) {
  return observed.has(required) || (required === "file.read" && observed.has("file.read_many"));
}

function stableJson(value) {
  if (Array.isArray(value)) return value.map(stableJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableJson(value[key])])
    );
  }
  return value;
}

function postconditionSubjectSha256(kind, subject) {
  return sha256(
    Buffer.concat([
      postconditionSubjectDomain,
      Buffer.from(kind),
      Buffer.from([0]),
      Buffer.from(subject)
    ])
  );
}

function frozenPostconditions(testCase) {
  const verification = testCase.verification;
  const receipt = (kind, subject, expected) => ({
    kind,
    subject_sha256: postconditionSubjectSha256(kind, subject),
    expected_sha256: sha256(expected)
  });
  return [
    ...(verification.json_files || []).map((check) =>
      receipt("json_file", check.path, JSON.stringify(stableJson(check.equals)))
    ),
    ...(verification.exact_files || []).map((check) =>
      receipt("exact_file", check.path, check.content)
    ),
    ...(verification.file_contains || []).map((check) =>
      receipt("file_contains", check.path, JSON.stringify(check.values))
    ),
    ...(verification.commands || []).map((check) =>
      receipt(
        "command",
        JSON.stringify(stableJson({ program: check.program, args: check.args })),
        check.stdout_contains
      )
    )
  ];
}

function validateFixtureReceipt(run, testCase, key) {
  const browserContract = testCase.verification.browser_target_receipt;
  if (!browserContract) {
    requireFact(run.fixture_receipt === null, `${key}: non-browser run must not claim an HTTP fixture`);
    requireFact(
      run.verification.expected_browser_target_sha256 === null,
      `${key}: non-browser run must not claim a browser target`
    );
    return null;
  }
  if (run.fixture_receipt === null) {
    requireFact(
      run.verification.expected_browser_target_sha256 === null,
      `${key}: missing HTTP fixture cannot claim a browser target`
    );
    requireFact(
      !run.verification.external_effect_passed && !run.verification.quality_passed,
      `${key}: browser success is missing its HTTP fixture receipt`
    );
    return null;
  }
  exactFields(
    run.fixture_receipt,
    ["target_sha256", "body_sha256", "successful_requests"],
    `${key}: HTTP fixture receipt`
  );
  requireFact(sha256Pattern.test(run.fixture_receipt.target_sha256), `${key}: fixture target hash is invalid`);
  requireFact(sha256Pattern.test(run.fixture_receipt.body_sha256), `${key}: fixture body hash is invalid`);
  nonNegativeInteger(run.fixture_receipt.successful_requests, `${key}: fixture request count`);
  const fixture = testCase.files.find((candidate) => candidate.path === "site/index.html");
  requireFact(
    run.fixture_receipt.body_sha256 === sha256(Buffer.from(fixture.content)),
    `${key}: HTTP fixture body hash mismatch`
  );
  if (isOracleTreatment(run.treatment)) {
    requireFact(
      run.verification.expected_browser_target_sha256 === null,
      `${key}: Direct must not claim product browser-target verification`
    );
  } else {
    requireFact(
      run.verification.expected_browser_target_sha256 === run.fixture_receipt.target_sha256,
      `${key}: expected browser target is not bound to the HTTP fixture`
    );
  }
  return run.fixture_receipt;
}

function validateToolReceipts(run, testCase, key, fixtureReceipt) {
  requireFact(Array.isArray(run.tool_receipts), `${key}: tool receipts are missing`);
  const seenCalls = new Set();
  const successfulTools = new Set();
  const statusCounts = Object.fromEntries(toolReceiptStatuses.map((status) => [status, 0]));
  for (const [index, receipt] of run.tool_receipts.entries()) {
    const label = `${key}: tool receipt ${index}`;
    exactFields(
      receipt,
      [
        "call_sha256",
        "tool",
        "status",
        "input_fingerprint",
        "started_sequence",
        "finished_sequence",
        "target_sha256",
        "evidence_sha256",
        "artifacts"
      ],
      label
    );
    requireFact(sha256Pattern.test(receipt.call_sha256), `${label} call hash is invalid`);
    requireFact(!seenCalls.has(receipt.call_sha256), `${key}: duplicate tool call receipt`);
    seenCalls.add(receipt.call_sha256);
    requireFact(typeof receipt.tool === "string" && receipt.tool.length > 0, `${label} tool is invalid`);
    requireFact(toolReceiptStatuses.includes(receipt.status), `${label} status is invalid`);
    nullableSha256(receipt.input_fingerprint, `${label} input fingerprint`);
    for (const field of ["started_sequence", "finished_sequence"]) {
      requireFact(
        receipt[field] === null || (Number.isInteger(receipt[field]) && receipt[field] >= 0),
        `${label} ${field} is invalid`
      );
    }
    requireFact(
      receipt.status !== "incomplete" || receipt.finished_sequence === null,
      `${label} incomplete call cannot have a finished sequence`
    );
    if (receipt.status !== "invalid") {
      requireFact(receipt.input_fingerprint !== null, `${label} typed input fingerprint is missing`);
    }
    if (["succeeded", "failed", "cancelled", "denied", "superseded"].includes(receipt.status)) {
      requireFact(
        receipt.started_sequence !== null && receipt.finished_sequence !== null,
        `${label} terminal status lacks a complete lifecycle`
      );
    } else if (receipt.status === "incomplete") {
      requireFact(receipt.started_sequence !== null, `${label} incomplete call was never proposed or started`);
    }
    if (receipt.started_sequence !== null && receipt.finished_sequence !== null) {
      requireFact(
        receipt.started_sequence < receipt.finished_sequence,
        `${label} lifecycle sequence is invalid`
      );
    }
    nullableSha256(receipt.target_sha256, `${label} target hash`);
    nullableSha256(receipt.evidence_sha256, `${label} evidence hash`);
    requireFact(Array.isArray(receipt.artifacts), `${label} artifacts are missing`);
    for (const [artifactIndex, artifact] of receipt.artifacts.entries()) {
      const artifactLabel = `${label} artifact ${artifactIndex}`;
      exactFields(
        artifact,
        ["path_sha256", "content_sha256", "bytes", "mime_type"],
        artifactLabel
      );
      requireFact(sha256Pattern.test(artifact.path_sha256), `${artifactLabel} path hash is invalid`);
      requireFact(sha256Pattern.test(artifact.content_sha256), `${artifactLabel} content hash is invalid`);
      nonNegativeInteger(artifact.bytes, `${artifactLabel} bytes`);
      requireFact(
        artifact.mime_type === null ||
          (typeof artifact.mime_type === "string" && artifact.mime_type.length > 0),
        `${artifactLabel} MIME type is invalid`
      );
    }
    statusCounts[receipt.status] += 1;
    if (receipt.status === "succeeded") successfulTools.add(receipt.tool);
  }

  requireFact(run.metrics.tool_calls === run.tool_receipts.length, `${key}: tool-call denominator mismatch`);
  for (const [status, field] of Object.entries(toolReceiptMetricFields)) {
    requireFact(run.metrics[field] === statusCounts[status], `${key}: ${field} does not match receipts`);
  }
  requireFact(
    new Set(run.tools_used).size === run.tools_used.length &&
      JSON.stringify([...new Set(run.tools_used)].sort()) ===
        JSON.stringify([...successfulTools].sort()),
    `${key}: tools_used must contain only the successful tool set`
  );
  requireFact(
    !isOracleTreatment(run.treatment) || run.tool_receipts.length === 0,
    `${key}: Direct must not claim product tool receipts`
  );

  if (isOracleTreatment(run.treatment)) return;
  const requiredAny = testCase.verification.required_tools_any;
  const requiredAll = testCase.verification.required_tools_all;
  const requiredToolsPassed =
    (requiredAny.length === 0 ||
      requiredAny.some((required) => toolRequirementSatisfied(successfulTools, required))) &&
    requiredAll.every((required) => toolRequirementSatisfied(successfulTools, required));
  if (run.verification.external_effect_passed || run.verification.quality_passed) {
    requireFact(requiredToolsPassed, `${key}: required tools lack successful receipts`);
  }

  const browserContract = testCase.verification.browser_target_receipt;
  if (!browserContract || !(run.verification.external_effect_passed || run.verification.quality_passed)) {
    return;
  }
  requireFact(fixtureReceipt !== null, `${key}: browser success is missing its HTTP fixture receipt`);
  requireFact(fixtureReceipt.successful_requests > 0, `${key}: browser fixture received no successful request`);
  const successfulAtTarget = run.tool_receipts.filter(
    (receipt) =>
      receipt.status === "succeeded" && receipt.target_sha256 === fixtureReceipt.target_sha256
  );
  requireFact(
    browserContract.tools_all.every((tool) =>
      successfulAtTarget.some((receipt) => receipt.tool === tool)
    ),
    `${key}: browser target tools_all lacks an exact-target success receipt`
  );
  requireFact(
    successfulAtTarget.some(
      (receipt) =>
        browserContract.tools_any.includes(receipt.tool) &&
        receipt.evidence_sha256 !== null &&
        receipt.artifacts.length >= browserContract.minimum_artifacts
    ),
    `${key}: browser evidence lacks an exact-target artifact receipt`
  );
}

function validatePostconditionReceipts(run, testCase, key) {
  const verification = run.verification;
  requireFact(
    Array.isArray(verification.postcondition_receipts),
    `${key}: postcondition receipts are missing`
  );
  const interrupted = ["infrastructure_failed", "timed_out", "running"].includes(
    run.terminal_status
  );
  const expected = isOracleTreatment(run.treatment) || interrupted
    ? []
    : frozenPostconditions(testCase);
  requireFact(
    verification.postcondition_receipts.length === expected.length,
    `${key}: postcondition receipt count drifted from the frozen suite`
  );
  let passed = 0;
  for (const [index, receipt] of verification.postcondition_receipts.entries()) {
    const label = `${key}: postcondition receipt ${index}`;
    exactFields(
      receipt,
      [
        "kind",
        "subject_sha256",
        "expected_sha256",
        "observed_sha256",
        "artifact_sha256",
        "bytes",
        "passed"
      ],
      label
    );
    requireFact(typeof receipt.kind === "string" && receipt.kind.length > 0, `${label} kind is invalid`);
    requireFact(sha256Pattern.test(receipt.subject_sha256), `${label} subject hash is invalid`);
    requireFact(sha256Pattern.test(receipt.expected_sha256), `${label} expected hash is invalid`);
    nullableSha256(receipt.observed_sha256, `${label} observed hash`);
    nullableSha256(receipt.artifact_sha256, `${label} artifact hash`);
    requireFact(
      receipt.bytes === null || (Number.isInteger(receipt.bytes) && receipt.bytes >= 0),
      `${label} bytes is invalid`
    );
    requireFact(typeof receipt.passed === "boolean", `${label} passed flag is invalid`);
    requireFact(receipt.kind === expected[index].kind, `${label} kind drifted from the frozen suite`);
    requireFact(
      receipt.subject_sha256 === expected[index].subject_sha256,
      `${label} subject drifted from the frozen suite`
    );
    requireFact(
      receipt.expected_sha256 === expected[index].expected_sha256,
      `${label} expectation drifted from the frozen suite`
    );
    requireFact(
      (receipt.artifact_sha256 === null) === (receipt.bytes === null),
      `${label} artifact digest and byte count are inconsistent`
    );
    if (receipt.passed) {
      requireFact(
        receipt.observed_sha256 !== null &&
          receipt.artifact_sha256 !== null &&
          receipt.bytes !== null,
        `${label} passed result lacks observed artifact evidence`
      );
      if (["json_file", "exact_file"].includes(receipt.kind)) {
        requireFact(
          receipt.observed_sha256 === receipt.expected_sha256,
          `${label} equality postcondition does not match the expectation`
        );
      }
    }
    if (receipt.passed) passed += 1;
  }
  nonNegativeInteger(verification.passed_checks, `${key}: passed checks`);
  nonNegativeInteger(verification.total_checks, `${key}: total checks`);
  requireFact(passed <= verification.passed_checks, `${key}: postcondition passes exceed passed checks`);
  requireFact(
    verification.postcondition_receipts.length <= verification.total_checks,
    `${key}: postcondition receipts exceed total checks`
  );
  if (verification.quality_passed || verification.external_effect_passed === true) {
    requireFact(
      verification.postcondition_receipts.every((receipt) => receipt.passed),
      `${key}: quality pass lacks successful postcondition receipts`
    );
  }
}

export function validateReceiptEvidence(run, testCase, key) {
  validatePostconditionReceipts(run, testCase, key);
  const fixtureReceipt = validateFixtureReceipt(run, testCase, key);
  validateToolReceipts(run, testCase, key, fixtureReceipt);
}

function sanitizeFixtureReceipt(receipt) {
  return receipt
    ? {
        target_sha256: receipt.target_sha256,
        body_sha256: receipt.body_sha256,
        successful_requests: receipt.successful_requests
      }
    : null;
}

function sanitizeToolReceipt(receipt) {
  return {
    call_sha256: receipt.call_sha256,
    tool: receipt.tool,
    status: receipt.status,
    input_fingerprint: receipt.input_fingerprint,
    started_sequence: receipt.started_sequence,
    finished_sequence: receipt.finished_sequence,
    target_sha256: receipt.target_sha256,
    evidence_sha256: receipt.evidence_sha256,
    artifacts: receipt.artifacts.map((artifact) => ({
      path_sha256: artifact.path_sha256,
      content_sha256: artifact.content_sha256,
      bytes: artifact.bytes,
      mime_type: artifact.mime_type
    }))
  };
}

function sanitizePostconditionReceipt(receipt) {
  return {
    kind: receipt.kind,
    subject_sha256: receipt.subject_sha256,
    expected_sha256: receipt.expected_sha256,
    observed_sha256: receipt.observed_sha256,
    artifact_sha256: receipt.artifact_sha256,
    bytes: receipt.bytes,
    passed: receipt.passed
  };
}

function sanitizeVerification(verification) {
  return {
    quality_passed: verification.quality_passed,
    answer_passed: verification.answer_passed,
    external_effect_passed: verification.external_effect_passed,
    passed_checks: verification.passed_checks,
    total_checks: verification.total_checks,
    safety_violations: verification.safety_violations,
    failure_count: verification.failures.length,
    expected_browser_target_sha256: verification.expected_browser_target_sha256,
    postcondition_receipts: verification.postcondition_receipts.map(
      sanitizePostconditionReceipt
    )
  };
}

export function sanitizeReceiptEvidence(run) {
  return {
    fixtureReceipt: sanitizeFixtureReceipt(run.fixture_receipt),
    toolReceipts: run.tool_receipts.map(sanitizeToolReceipt),
    verification: sanitizeVerification(run.verification)
  };
}
