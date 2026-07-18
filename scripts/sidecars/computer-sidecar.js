#!/usr/bin/env node

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const REQUEST_SCHEMA = "cindx.computer-control.v1";
const RESPONSE_SCHEMA = "cindx.computer-control-result.v1";
const OSASCRIPT = process.env.CINDX_OSASCRIPT || "/usr/bin/osascript";
const SCREENCAPTURE = process.env.CINDX_SCREENCAPTURE || "/usr/sbin/screencapture";

function readRequest(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function run(command, args) {
  return spawnSync(command, args, {
    encoding: "utf8",
    timeout: 15_000,
    maxBuffer: 1024 * 1024
  });
}

function commandError(command, result) {
  if (result.error) return `${command} failed: ${result.error.message}`;
  const detail = String(result.stderr || result.stdout || "").trim();
  return detail ? `${command} failed: ${detail}` : `${command} exited with ${result.status}`;
}

function ensureCommandSucceeded(command, result) {
  if (result.status !== 0 || result.error) throw new Error(commandError(command, result));
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
  const modifierClause = modifiers.length > 0 ? ` using {${modifiers.join(", ")}}` : "";
  const keyCodes = {
    enter: 36,
    return: 36,
    tab: 48,
    space: 49,
    escape: 53,
    esc: 53,
    backspace: 51,
    delete: 51,
    forwarddelete: 117,
    left: 123,
    right: 124,
    down: 125,
    up: 126,
    home: 115,
    end: 119,
    pageup: 116,
    pagedown: 121
  };
  const keyCode = keyCodes[main.toLowerCase().replace(/[ _-]+/g, "")];
  const action = keyCode === undefined
    ? `keystroke "${quoteAppleScript(main)}"${modifierClause}`
    : `key code ${keyCode}${modifierClause}`;
  return `tell application "System Events" to ${action}`;
}

function finiteNumber(value, label, fallback) {
  if ((value === undefined || value === null || value === "") && fallback !== undefined) {
    return fallback;
  }
  const number = Number(value);
  if (!Number.isFinite(number)) throw new Error(`${label} must be a finite number`);
  return number;
}

function mouseClickScript(x, y, button) {
  const right = button === "right";
  return `ObjC.import("CoreGraphics");
const point = {x:${x}, y:${y}};
const down = $.CGEventCreateMouseEvent(null, ${right ? "$.kCGEventRightMouseDown" : "$.kCGEventLeftMouseDown"}, point, ${right ? "$.kCGMouseButtonRight" : "$.kCGMouseButtonLeft"});
const up = $.CGEventCreateMouseEvent(null, ${right ? "$.kCGEventRightMouseUp" : "$.kCGEventLeftMouseUp"}, point, ${right ? "$.kCGMouseButtonRight" : "$.kCGMouseButtonLeft"});
if (!down || !up) throw new Error("unable to create mouse event");
$.CGEventPost($.kCGHIDEventTap, down);
$.CGEventPost($.kCGHIDEventTap, up);`;
}

function scrollScript(deltaX, deltaY) {
  return `ObjC.import("CoreGraphics");
const event = $.CGEventCreateScrollWheelEvent(null, $.kCGScrollEventUnitPixel, 2, ${-deltaY}, ${-deltaX});
if (!event) throw new Error("unable to create scroll event");
$.CGEventPost($.kCGHIDEventTap, event);`;
}

function screenshotArgs(request, artifactPath) {
  const args = ["-x"];
  if (request.region) {
    if (!/^-?\d+,-?\d+,\d+,\d+$/.test(String(request.region).trim())) {
      throw new Error("region must be x,y,width,height");
    }
    args.push("-R", String(request.region).trim());
  }
  args.push(artifactPath);
  return args;
}

function health() {
  if (process.platform !== "darwin") throw new Error("computer-sidecar currently requires macOS");
  for (const command of [OSASCRIPT, SCREENCAPTURE]) {
    if (!fs.existsSync(command)) throw new Error(`required command is unavailable: ${command}`);
  }
  const accessibility = run(OSASCRIPT, [
    "-e",
    'tell application "System Events" to get UI elements enabled'
  ]);
  ensureCommandSucceeded(OSASCRIPT, accessibility);
  if (String(accessibility.stdout).trim().toLowerCase() !== "true") {
    throw new Error("macOS Accessibility permission is not enabled");
  }

  const probeDir = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-computer-health-"));
  const probePath = path.join(probeDir, "capture.png");
  try {
    const capture = run(SCREENCAPTURE, ["-x", probePath]);
    ensureCommandSucceeded(SCREENCAPTURE, capture);
    if (!fs.existsSync(probePath) || fs.statSync(probePath).size === 0) {
      throw new Error("macOS Screen Recording permission did not produce a screenshot");
    }
  } finally {
    fs.rmSync(probeDir, { recursive: true, force: true });
  }
  process.stdout.write("computer-sidecar ok accessibility=true screen_capture=true\n");
}

function execute(request, requestPath) {
  const startedAt = Date.now();
  if (request.schema && request.schema !== REQUEST_SCHEMA) {
    throw new Error(`unsupported schema ${request.schema}`);
  }
  const action = request.action;
  const requestDir = path.dirname(requestPath);
  const artifacts = [];
  let output = "";

  if (action === "screenshot") {
    const artifactPath = path.join(requestDir, `${request.id || "computer-action"}.png`);
    const result = run(SCREENCAPTURE, screenshotArgs(request, artifactPath));
    ensureCommandSucceeded(SCREENCAPTURE, result);
    if (!fs.existsSync(artifactPath) || fs.statSync(artifactPath).size === 0) {
      throw new Error("screenshot command completed without producing pixels");
    }
    artifacts.push({ path: artifactPath, mime_type: "image/png", title: "Desktop screenshot" });
    output = `desktop screenshot captured\nscreenshot=${artifactPath}`;
  } else if (action === "click") {
    const x = finiteNumber(request.x, "x");
    const y = finiteNumber(request.y, "y");
    const button = request.button || "left";
    if (!new Set(["left", "right"]).has(button)) throw new Error("button must be left or right");
    const result = run(OSASCRIPT, ["-l", "JavaScript", "-e", mouseClickScript(x, y, button)]);
    ensureCommandSucceeded(OSASCRIPT, result);
    output = `${button} clicked at ${x},${y}`;
  } else if (action === "type") {
    if (request.text === undefined || request.text === null) throw new Error("text is required");
    const result = run(OSASCRIPT, [
      "-e",
      'on run argv\n  tell application "System Events" to keystroke (item 1 of argv)\nend run',
      "--",
      String(request.text)
    ]);
    ensureCommandSucceeded(OSASCRIPT, result);
    output = `typed ${String(request.text).length} characters`;
  } else if (action === "key") {
    if (!request.key) throw new Error("key is required");
    const result = run(OSASCRIPT, ["-e", keyScript(request.key)]);
    ensureCommandSucceeded(OSASCRIPT, result);
    output = `pressed ${request.key}`;
  } else if (action === "scroll") {
    const deltaX = finiteNumber(request.delta_x, "delta_x", 0);
    const deltaY = finiteNumber(request.delta_y, "delta_y", 600);
    const result = run(OSASCRIPT, ["-l", "JavaScript", "-e", scrollScript(deltaX, deltaY)]);
    ensureCommandSucceeded(OSASCRIPT, result);
    output = `scrolled ${deltaX},${deltaY}`;
  } else {
    throw new Error(`unsupported computer action ${action || "unknown"}`);
  }

  return {
    schema: RESPONSE_SCHEMA,
    ok: true,
    id: request.id,
    action,
    controller: "native_macos",
    output,
    artifacts,
    duration_ms: Date.now() - startedAt
  };
}

function main() {
  if (process.argv.includes("--health")) {
    health();
    return;
  }
  const requestPath = process.argv[2];
  if (!requestPath) throw new Error("missing request path");
  const response = execute(readRequest(requestPath), requestPath);
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error.stack || error.message || error}\n`);
  process.exit(1);
}
