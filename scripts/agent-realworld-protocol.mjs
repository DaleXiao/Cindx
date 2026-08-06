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
