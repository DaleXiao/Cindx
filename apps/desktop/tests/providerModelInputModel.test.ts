import assert from "node:assert/strict";
import test from "node:test";
import {
  filterProviderModelOptions,
  moveProviderModelOptionIndex,
  validProviderModelOptionIndex
} from "../src/components/providerModelInputModel.ts";

test("editable model combobox filters suggestions without rejecting a custom ID", () => {
  const options = ["gpt-4.1", "gpt-image-2", "gpt-realtime"];
  assert.deepEqual(filterProviderModelOptions(options, ""), options);
  assert.deepEqual(filterProviderModelOptions(options, "IMAGE"), ["gpt-image-2"]);
  assert.deepEqual(filterProviderModelOptions(options, "private-deployment"), []);
});

test("editable model combobox keyboard navigation wraps through visible options", () => {
  assert.equal(moveProviderModelOptionIndex(-1, 3, "next"), 0);
  assert.equal(moveProviderModelOptionIndex(-1, 3, "previous"), 2);
  assert.equal(moveProviderModelOptionIndex(2, 3, "next"), 0);
  assert.equal(moveProviderModelOptionIndex(0, 3, "previous"), 2);
  assert.equal(moveProviderModelOptionIndex(0, 0, "next"), -1);
});

test("editable model combobox drops an active option that disappeared after refresh", () => {
  assert.equal(validProviderModelOptionIndex(1, 3), 1);
  assert.equal(validProviderModelOptionIndex(2, 2), -1);
  assert.equal(validProviderModelOptionIndex(-1, 2), -1);
  assert.equal(validProviderModelOptionIndex(0, 0), -1);
});
