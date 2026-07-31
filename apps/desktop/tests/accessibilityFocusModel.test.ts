import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  permissionFocusTarget,
  wrappedDialogFocusIndex
} from "../src/components/accessibilityFocusModel.ts";

function relativeLuminance(color: string) {
  const components = [1, 3, 5].map(
    (index) => Number.parseInt(color.slice(index, index + 2), 16) / 255
  );
  const [red, green, blue] = components.map((component) =>
    component <= 0.04045 ? component / 12.92 : ((component + 0.055) / 1.055) ** 2.4
  );
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrastRatio(foreground: string, background: string) {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  return (
    (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) /
    (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
  );
}

test("permission focus follows each new request and returns to the composer", () => {
  assert.equal(permissionFocusTarget(null, null, false), null);
  assert.equal(permissionFocusTarget(null, "request-a", false), "request");
  assert.equal(permissionFocusTarget("request-a", "request-a", true), null);
  assert.equal(permissionFocusTarget("request-a", "request-b", true), "request");
  assert.equal(permissionFocusTarget("request-b", null, true), "composer");
  assert.equal(permissionFocusTarget("request-b", null, false), null);
});

test("dialog focus wraps in both directions and recovers from outside focus", () => {
  assert.equal(wrappedDialogFocusIndex(0, 2, 1), 1);
  assert.equal(wrappedDialogFocusIndex(1, 2, 1), 0);
  assert.equal(wrappedDialogFocusIndex(0, 2, -1), 1);
  assert.equal(wrappedDialogFocusIndex(1, 2, -1), 0);
  assert.equal(wrappedDialogFocusIndex(-1, 2, 1), 0);
  assert.equal(wrappedDialogFocusIndex(-1, 2, -1), 1);
  assert.equal(wrappedDialogFocusIndex(0, 0, 1), null);
});

test("light subtle text remains AA-readable across supported surfaces", () => {
  const css = readFileSync(new URL("../src/styles/foundation.css", import.meta.url), "utf8");
  const lightRoot = css.match(/:root \{([\s\S]*?)\n\}/)?.[1] ?? "";
  const subtle = lightRoot.match(/--subtle:\s*(#[0-9a-f]{6})/i)?.[1];
  assert.ok(subtle, "light --subtle token must be defined");
  for (const background of ["#ffffff", "#fafafa", "#f5f5f5"]) {
    assert.ok(
      contrastRatio(subtle, background) >= 4.5,
      `${subtle} must reach 4.5:1 against ${background}`
    );
  }
});

test("dark subtle text remains AA-readable across supported surfaces", () => {
  const css = readFileSync(new URL("../src/styles/foundation.css", import.meta.url), "utf8");
  const darkRoot = css.match(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/)?.[1] ?? "";
  const readToken = (name: string) =>
    darkRoot.match(new RegExp(`${name}:\\s*(#[0-9a-f]{6})`, "i"))?.[1];
  const subtle = readToken("--subtle");
  assert.ok(subtle, "dark --subtle token must be defined");
  for (const token of [
    "--bg",
    "--panel",
    "--surface",
    "--surface-hover",
    "--surface-active"
  ]) {
    const background = readToken(token);
    assert.ok(background, `${token} must be defined in the dark theme`);
    assert.ok(
      contrastRatio(subtle, background) >= 4.5,
      `${subtle} must reach 4.5:1 against ${token} ${background}`
    );
  }
});
