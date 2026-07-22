import assert from "node:assert/strict";
import test from "node:test";
import {
  cindxVersionFromOrdinal,
  cindxVersionOrdinal,
  nextCindxVersion,
  parseCindxVersion
} from "./versioning.mjs";

test("ordinary builds increment the patch component", () => {
  assert.equal(nextCindxVersion("0.1.13"), "0.1.14");
  assert.equal(nextCindxVersion("2.4.99"), "2.4.100");
});

test("patch carry increments minor after 100", () => {
  assert.equal(nextCindxVersion("0.1.100"), "0.2.0");
});

test("minor carry increments major after 10", () => {
  assert.equal(nextCindxVersion("0.10.100"), "1.0.0");
});

test("CI build ordinals remain monotonic across carry boundaries", () => {
  const current = "0.1.13";
  const currentOrdinal = cindxVersionOrdinal(current);
  assert.equal(nextCindxVersion(current, currentOrdinal - 5), "0.1.14");
  assert.equal(nextCindxVersion(current, currentOrdinal + 100), "0.2.12");
  assert.equal(cindxVersionFromOrdinal(cindxVersionOrdinal("3.10.100")), "3.10.100");
});

test("non-canonical source versions are rejected", () => {
  assert.throws(() => parseCindxVersion("0.11.0"), /outside the supported range/);
  assert.throws(() => parseCindxVersion("0.1.101"), /outside the supported range/);
  assert.throws(() => parseCindxVersion("v1.0.0"), /Unsupported Cindx version/);
});
