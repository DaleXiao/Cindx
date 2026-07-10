#!/usr/bin/env node

const fs = require("fs");
const { spawnSync } = require("child_process");

function readRequest(path) {
  return JSON.parse(fs.readFileSync(path, "utf8"));
}

function run(command, args) {
  return spawnSync(command, args, { encoding: "utf8" });
}

if (process.argv.includes("--health")) {
  console.log("browser-sidecar ok");
  process.exit(0);
}

const requestPath = process.argv[2];
if (!requestPath) {
  console.error("missing request path");
  process.exit(2);
}

const request = readRequest(requestPath);
const action = request.action || "unknown";
const fields = [`controller=bundled-node`, `namespace=browser`, `action=${action}`, `request=${requestPath}`];

if (request.url) {
  const opened = run("/usr/bin/open", [request.url]);
  fields.push(`url=${request.url}`);
  fields.push(`open_status=${opened.status === 0 ? "ok" : "failed"}`);
}
if (request.selector) fields.push(`selector=${request.selector}`);
if (request.text) fields.push(`text_length=${String(request.text).length}`);
if (request.x && request.y) fields.push(`coordinates=${request.x},${request.y}`);
if (request.delta_x || request.delta_y) fields.push(`scroll=${request.delta_x || 0},${request.delta_y || 0}`);

console.log(`browser sidecar accepted request\n${fields.join("\n")}`);
