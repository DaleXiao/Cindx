import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sidecar = path.join(repoRoot, "scripts", "sidecars", "computer-sidecar.js");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-computer-sidecar-"));
const osascript = path.join(root, "fake-osascript.js");
const screencapture = path.join(root, "fake-screencapture.js");
const commandLog = path.join(root, "commands.jsonl");
let counter = 0;

fs.writeFileSync(
  osascript,
  `#!/usr/bin/env node
const fs = require("fs");
fs.appendFileSync(process.env.CINDX_TEST_COMMAND_LOG, JSON.stringify(process.argv.slice(2)) + "\\n");
if (process.env.CINDX_TEST_NATIVE_FAILURE === "1") {
  process.stderr.write("simulated native failure\\n");
  process.exit(7);
}
`,
  { mode: 0o700 }
);
fs.writeFileSync(
  screencapture,
  `#!/usr/bin/env node
const fs = require("fs");
const target = process.argv.at(-1);
fs.writeFileSync(target, Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
`,
  { mode: 0o700 }
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function invoke(action, fields = {}, extraEnv = {}) {
  counter += 1;
  const requestPath = path.join(root, `request-${counter}.json`);
  fs.writeFileSync(
    requestPath,
    `${JSON.stringify({
      schema: "cindx.computer-control.v1",
      id: `computer-test-${counter}`,
      action,
      ...fields
    })}\n`,
    { mode: 0o600 }
  );
  const result = spawnSync(process.execPath, [sidecar, requestPath], {
    cwd: repoRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      CINDX_OSASCRIPT: osascript,
      CINDX_SCREENCAPTURE: screencapture,
      CINDX_TEST_COMMAND_LOG: commandLog,
      ...extraEnv
    }
  });
  return {
    ...result,
    response: result.status === 0 ? JSON.parse(result.stdout) : null
  };
}

try {
  const click = invoke("click", { x: "0", y: "0", button: "right" });
  assert(click.status === 0, `zero-coordinate click failed: ${click.stderr}`);
  assert(click.response.ok, "click should return a successful structured response");

  const scroll = invoke("scroll", { delta_x: "12", delta_y: "640" });
  assert(scroll.status === 0, `scroll failed: ${scroll.stderr}`);
  const loggedCommands = fs.readFileSync(commandLog, "utf8");
  assert(
    loggedCommands.includes("CGEventCreateScrollWheelEvent"),
    "scroll should execute a CoreGraphics wheel event"
  );

  const screenshot = invoke("screenshot");
  assert(screenshot.status === 0, `screenshot failed: ${screenshot.stderr}`);
  assert(screenshot.response.artifacts.length === 1, "screenshot should return one artifact");
  assert(fs.statSync(screenshot.response.artifacts[0].path).size > 0, "screenshot must contain pixels");

  const failed = invoke("key", { key: "Enter" }, { CINDX_TEST_NATIVE_FAILURE: "1" });
  assert(failed.status !== 0, "native failure must produce a non-zero sidecar exit");
  assert(failed.stderr.includes("simulated native failure"), "native error should be preserved");

  process.stdout.write("computer sidecar integration ok\n");
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
