import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sidecar = path.join(repoRoot, "scripts", "sidecars", "browser-sidecar.js");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-browser-v2-"));
const sessionDir = path.join(root, "session");
const outputDir = path.join(root, "artifacts");
let requestCounter = 0;

const server = http.createServer((request, response) => {
  if (request.url === "/frame") {
    response.setHeader("content-type", "text/html; charset=utf-8");
    response.end(`<!doctype html><button aria-label="Frame action" onclick="document.querySelector('#frame-result').textContent='frame-clicked'">Frame action</button><div id="frame-result">frame-idle</div>`);
    return;
  }
  if (request.url === "/download") {
    response.setHeader("content-type", "text/plain; charset=utf-8");
    response.setHeader("content-disposition", "attachment; filename=fixture.txt");
    response.end("download-ok\n");
    return;
  }
  response.setHeader("content-type", "text/html; charset=utf-8");
  response.end(`<!doctype html>
    <label>Message <input aria-label="Message" id="message"></label>
    <div id="typed"></div>
    <button aria-label="Increment" id="increment" onclick="document.querySelector('#count').textContent=String(Number(document.querySelector('#count').textContent)+1)">Increment</button>
    <span id="count">0</span>
    <iframe name="child" src="/frame"></iframe>
    <a id="download" href="/download">Download fixture</a>
    <script>document.querySelector('#message').addEventListener('input', event => document.querySelector('#typed').textContent = event.target.value)</script>`);
});

async function invoke(action, fields = {}) {
  requestCounter += 1;
  const id = `integration-${requestCounter}`;
  process.stdout.write(`[browser-test] ${id} ${action}\n`);
  const requestPath = path.join(root, `${id}.json`);
  fs.writeFileSync(
    requestPath,
    `${JSON.stringify({
      schema: "cindx.browser-control.v2",
      id,
      action,
      session_id: "integration-session",
      session_dir: sessionDir,
      output_dir: outputDir,
      headless: "true",
      timeout_ms: "15000",
      ...fields
    })}\n`,
    { mode: 0o600 }
  );
  const result = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [sidecar, requestPath], {
      cwd: repoRoot,
      env: { ...process.env, CINDX_BROWSER_DEBUG: "1" },
      stdio: ["ignore", "pipe", "inherit"]
    });
    let stdout = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (stdout.length > 10 * 1024 * 1024) {
        child.kill();
        reject(new Error(`sidecar ${action} exceeded output limit`));
      }
    });
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout }));
  });
  fs.rmSync(requestPath, { force: true });
  if (result.status !== 0) {
    throw new Error(`sidecar ${action} failed: ${result.stdout}`);
  }
  const response = JSON.parse(result.stdout);
  if (!response.ok) throw new Error(`sidecar ${action} rejected: ${response.output}`);
  process.stdout.write(`[browser-test] ${id} ok ${response.duration_ms}ms\n`);
  return response;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
const baseUrl = `http://127.0.0.1:${address.port}`;

try {
  const opened = await invoke("open", { url: baseUrl });
  assert(opened.page?.id, "open should return a CDP target id");
  const statePath = path.join(sessionDir, "session-state.json");
  const browserPid = JSON.parse(fs.readFileSync(statePath, "utf8")).browser_pid;
  assert(processIsAlive(browserPid), "open should leave a live browser process");

  await invoke("type", { role: "textbox", name: "Message", text: "hello-cdp" });
  await invoke("click", { role: "button", name: "Increment", wait_for: "#count" });
  const text = await invoke("extract_text", { selector: "body" });
  assert(text.output.includes("hello-cdp"), "typed text should persist across sidecar processes");
  assert(text.output.includes("1"), "click should update the page");

  await invoke("click", { frame: "child", role: "button", name: "Frame action" });
  const frameText = await invoke("extract_text", { frame: "child", selector: "body" });
  assert(frameText.output.includes("frame-clicked"), "frame action should execute");

  const capture = await invoke("capture");
  assert(capture.artifacts.length === 2, "capture should return screenshot and text artifacts");
  for (const artifact of capture.artifacts) {
    assert(fs.existsSync(artifact.path), `missing artifact ${artifact.path}`);
  }

  await invoke("open", { url: `${baseUrl}/?second`, new_tab: "true" });
  const tabs = await invoke("tabs");
  assert(tabs.tabs.length === 2, "new_tab should preserve both tabs");
  await invoke("select_tab", { tab_id: opened.page.id });

  const download = await invoke("click", { selector: "#download", download: "true" });
  assert(download.artifacts.length === 1, "download click should return one artifact");
  assert(
    fs.readFileSync(download.artifacts[0].path, "utf8").includes("download-ok"),
    "download artifact should contain server data"
  );

  await invoke("close");
  assert(!processIsAlive(browserPid), "close should terminate the browser process");
  assert(!fs.existsSync(statePath), "close should remove stale browser session state");
  process.stdout.write("browser sidecar integration ok\n");
} finally {
  await new Promise((resolve) => server.close(resolve));
  try {
    const state = JSON.parse(fs.readFileSync(path.join(sessionDir, "session-state.json"), "utf8"));
    if (state.browser_pid) spawnSync("kill", [String(state.browser_pid)]);
  } catch {}
  await new Promise((resolve) => setTimeout(resolve, 500));
  fs.rmSync(root, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 });
}
