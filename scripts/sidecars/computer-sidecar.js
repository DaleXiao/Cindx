#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

function readRequest(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function run(command, args) {
  return spawnSync(command, args, { encoding: "utf8" });
}

function quoteAppleScript(value) {
  return String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function keyScript(key) {
  const parts = String(key)
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  const main = parts.pop() || key;
  const modifiers = parts
    .map((part) => part.toLowerCase())
    .map((part) => {
      if (part === "cmd" || part === "command") return "command down";
      if (part === "ctrl" || part === "control") return "control down";
      if (part === "alt" || part === "option") return "option down";
      if (part === "shift") return "shift down";
      return null;
    })
    .filter(Boolean);
  if (modifiers.length === 0) return `tell application "System Events" to keystroke "${quoteAppleScript(main)}"`;
  return `tell application "System Events" to keystroke "${quoteAppleScript(main)}" using {${modifiers.join(", ")}}`;
}

if (process.argv.includes("--health")) {
  console.log("computer-sidecar ok");
  process.exit(0);
}

const requestPath = process.argv[2];
if (!requestPath) {
  console.error("missing request path");
  process.exit(2);
}

const request = readRequest(requestPath);
const action = request.action || "unknown";
const requestDir = path.dirname(requestPath);
const artifactPath = path.join(requestDir, `${request.id || "computer-action"}.png`);
const fields = [`controller=bundled-node`, `namespace=computer`, `action=${action}`, `request=${requestPath}`];

let result = { status: 0, stderr: "" };
if (action === "screenshot") {
  result = run("/usr/sbin/screencapture", ["-x", artifactPath]);
  fields.push(`artifact=${artifactPath}`);
} else if (action === "click" && request.x && request.y) {
  result = run("/usr/bin/osascript", [
    "-e",
    `tell application "System Events" to click at {${Number(request.x)}, ${Number(request.y)}}`
  ]);
  fields.push(`coordinates=${request.x},${request.y}`);
} else if (action === "type" && request.text) {
  result = run("/usr/bin/osascript", [
    "-e",
    `tell application "System Events" to keystroke "${quoteAppleScript(request.text)}"`
  ]);
  fields.push(`text_length=${String(request.text).length}`);
} else if (action === "key" && request.key) {
  result = run("/usr/bin/osascript", ["-e", keyScript(request.key)]);
  fields.push(`key=${request.key}`);
} else if (action === "scroll") {
  fields.push(`scroll=${request.delta_x || 0},${request.delta_y || 600}`);
  fields.push("scroll_status=recorded");
}

if (result.status !== 0) {
  fields.push("native_status=failed");
  fields.push(`native_error=${String(result.stderr || "").trim()}`);
} else {
  fields.push("native_status=ok");
}

console.log(`computer sidecar accepted request\n${fields.join("\n")}`);
