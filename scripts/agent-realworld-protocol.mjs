export const EXECUTION_ORDER_PROTOCOL = "cyclic_latin_square_v1";

const protocols = Object.freeze({
  "cindx.agent-realworld-suite.v3": Object.freeze({
    suiteSchema: "cindx.agent-realworld-suite.v3",
    rawSchema: "cindx.agent-realworld-raw.v3",
    sanitizedSchema: "cindx.agent-realworld-sanitized.v3",
    treatments: Object.freeze(["direct", "fast", "auto", "pro"]),
    oracleReference: "direct",
    groundedDirect: null,
    claimContract: "promotion_v1"
  }),
  "cindx.agent-realworld-suite.v4": Object.freeze({
    suiteSchema: "cindx.agent-realworld-suite.v4",
    rawSchema: "cindx.agent-realworld-raw.v4",
    sanitizedSchema: "cindx.agent-realworld-sanitized.v4",
    treatments: Object.freeze(["oracle_reference", "grounded_direct", "auto", "pro"]),
    oracleReference: "oracle_reference",
    groundedDirect: "grounded_direct",
    claimContract: "claim_contract_v2"
  }),
  "cindx.agent-memory-effect-suite.v1": Object.freeze({
    suiteSchema: "cindx.agent-memory-effect-suite.v1",
    rawSchema: "cindx.agent-memory-effect-raw.v1",
    sanitizedSchema: "cindx.agent-memory-effect-sanitized.v1",
    treatments: Object.freeze(["memory_on", "memory_off"]),
    oracleReference: null,
    groundedDirect: null,
    claimContract: "memory_effect_v1"
  })
});

export function protocolForSuite(suite) {
  const protocol = protocols[suite?.schema];
  if (!protocol) throw new Error("suite schema mismatch");
  return protocol;
}

export function isOracleReference(suite, treatment) {
  return treatment === protocolForSuite(suite).oracleReference;
}

export function isCurrentClaimProtocol(suite) {
  return protocolForSuite(suite).claimContract === "claim_contract_v2";
}

export function isMemoryEffectProtocol(suite) {
  return protocolForSuite(suite).claimContract === "memory_effect_v1";
}

function requireFact(condition, message) {
  if (!condition) throw new Error(message);
}

export function validatePreflight({ suite, gitHead, status, requestedReplicates }) {
  const protocol = protocolForSuite(suite);
  requireFact(
    JSON.stringify(suite.treatments) === JSON.stringify(protocol.treatments),
    "suite treatments do not match the frozen protocol order"
  );
  requireFact(
    suite.execution_order?.protocol === EXECUTION_ORDER_PROTOCOL &&
      JSON.stringify(suite.execution_order.base_treatments) ===
        JSON.stringify(suite.treatments),
    "suite execution-order contract mismatch"
  );
  if (protocol.claimContract === "memory_effect_v1") {
    const contract = suite.memory_effect_contract_v1;
    requireFact(
      contract?.schema === "cindx.agent-memory-effect-contract.v1" &&
        contract.on === "memory_on" &&
        contract.off === "memory_off" &&
        contract.required_cases === 2 &&
        contract.irrelevant_controls === 1 &&
        contract.replicates === 3 &&
        contract.cells === 18,
      "suite memory-effect contract mismatch"
    );
    requireFact(
      suite.default_replicates === 3 && suite.cases?.length === 3,
      "memory-effect protocol must freeze three cases and three replicates"
    );
  } else if (protocol.claimContract === "claim_contract_v2") {
    requireFact(
      suite.claim_contract_v2?.schema === "cindx.agent-realworld-claims.v2" &&
        suite.claim_contract_v2.baseline === protocol.groundedDirect,
      "suite claim contract mismatch"
    );
  } else {
    requireFact(
      suite.promotion_v1?.schema === "cindx.agent-realworld-promotion.v1" &&
        suite.promotion_v1.baseline === "fast",
      "suite promotion contract mismatch"
    );
  }
  requireFact(
    Number.isInteger(suite.per_run_timeout_seconds) &&
      suite.per_run_timeout_seconds >= 60 &&
      suite.per_run_timeout_seconds <= 3600,
    "suite per-run timeout must be between 60 and 3600 seconds"
  );
  requireFact(/^[0-9a-f]{40}$/.test(gitHead), "Git HEAD must be a full lowercase SHA");
  requireFact(!status.trim(), "worktree must be clean before provider-backed evaluation");
  const replicates = requestedReplicates
    ? Number.parseInt(requestedReplicates, 10)
    : suite.default_replicates;
  requireFact(Number.isInteger(replicates) && replicates > 0, "replicates must be positive");
  if (protocol.claimContract === "memory_effect_v1") {
    requireFact(replicates === 3, "memory-effect protocol requires exactly three replicates");
  }
  return { replicates };
}
