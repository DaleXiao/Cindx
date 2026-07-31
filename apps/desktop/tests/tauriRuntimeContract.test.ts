import assert from "node:assert/strict";
import test from "node:test";
import contract from "../contracts/tauri-dto-v1.json" with { type: "json" };
import { decodeNativeRuntimeStatus } from "../src/tauriRuntimeContract.ts";

const runtimeStatus = contract.cases.find((entry) => entry.name === "RuntimeStatus")
  ?.values[0];

test("native runtime status accepts the committed Rust wire contract", () => {
  assert.ok(runtimeStatus);
  assert.equal(decodeNativeRuntimeStatus(runtimeStatus), runtimeStatus);
});

test("native runtime status rejects preview null budgets and malformed arrays", () => {
  assert.ok(runtimeStatus);
  assert.throws(
    () => decodeNativeRuntimeStatus({ ...runtimeStatus, agentRunBudgets: null }),
    /RuntimeStatus\.agentRunBudgets must be an object/
  );
  assert.throws(
    () => decodeNativeRuntimeStatus({ ...runtimeStatus, registeredTools: ["file.read", 7] }),
    /RuntimeStatus\.registeredTools\[1\] must be a string/
  );
});
