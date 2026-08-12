import assert from "node:assert/strict";
import test from "node:test";
import {
  applyCustomCommandTemplate,
  customCommandRequiresArguments,
  CUSTOM_COMMAND_ARGUMENT_TOKEN
} from "../src/customCommandsModel.ts";

test("command with the argument token substitutes the trimmed composer text", () => {
  const template = "Run $ARGUMENTS with coverage.";
  assert.equal(
    applyCustomCommandTemplate(template, "  the test suite "),
    "Run the test suite with coverage."
  );
});

test("every occurrence of the argument token is substituted", () => {
  const template = "$ARGUMENTS before and $ARGUMENTS after";
  assert.equal(applyCustomCommandTemplate(template, "x"), "x before and x after");
});

test("an empty argument token expands to an empty string", () => {
  const template = "Prefix $ARGUMENTS suffix";
  assert.equal(applyCustomCommandTemplate(template, ""), "Prefix  suffix");
});

test("command without the token and empty composer yields the template", () => {
  assert.equal(applyCustomCommandTemplate("Summarize the workspace.", ""), "Summarize the workspace.");
});

test("command without the token preserves existing composer text after the template", () => {
  assert.equal(
    applyCustomCommandTemplate("Summarize the workspace.", "my notes"),
    "Summarize the workspace.\n\nmy notes"
  );
});

test("requires-arguments reflects the token presence", () => {
  assert.equal(customCommandRequiresArguments(`Do ${CUSTOM_COMMAND_ARGUMENT_TOKEN}`), true);
  assert.equal(customCommandRequiresArguments("No token here."), false);
});
