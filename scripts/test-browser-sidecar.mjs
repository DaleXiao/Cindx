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

fs.mkdirSync(sessionDir, { recursive: true });
const abandonedLockPath = path.join(sessionDir, ".action-lock.json");
fs.writeFileSync(abandonedLockPath, "{}\n", { mode: 0o600 });
const abandonedAt = new Date(Date.now() - 10_000);
fs.utimesSync(abandonedLockPath, abandonedAt, abandonedAt);

const staleSessionDir = path.join(root, "stale-session");
const staleStatePath = path.join(staleSessionDir, "session-state.json");
const staleProfileDir = path.join(staleSessionDir, "profile");
const staleCreatedAt = Date.now() - 60_000;
fs.mkdirSync(staleProfileDir, { recursive: true });
fs.writeFileSync(
  staleStatePath,
  `${JSON.stringify({
    schema: "cindx.browser-session.v1",
    session_id: "stale-session",
    browser_pid: 2_147_483_647,
    watchdog_pid: null,
    executable: "/nonexistent/cindx-test-browser",
    profile_dir: staleProfileDir,
    launch_token: "stale-launch",
    active_tab_id: null,
    created_at_ms: staleCreatedAt,
    last_used_at_ms: staleCreatedAt,
    lease_expires_at_ms: 1
  })}\n`,
  { mode: 0o600 }
);

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

async function invokeRaw(action, fields = {}) {
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
  return JSON.parse(result.stdout);
}

async function invoke(action, fields = {}) {
  const response = await invokeRaw(action, fields);
  if (!response.ok) throw new Error(`sidecar ${action} rejected: ${response.output}`);
  process.stdout.write(`[browser-test] ${response.id} ok ${response.duration_ms}ms\n`);
  return response;
}

async function assertCorruptStateRejected(name, prepareState, expectedError) {
  const corruptSessionDir = path.join(root, `corrupt-${name}`);
  const corruptStatePath = path.join(corruptSessionDir, "session-state.json");
  fs.mkdirSync(corruptSessionDir, { recursive: true });
  prepareState(corruptStatePath);
  const stateIsDirectory = fs.statSync(corruptStatePath).isDirectory();
  const originalState = stateIsDirectory ? null : fs.readFileSync(corruptStatePath, "utf8");
  try {
    const response = await invokeRaw("close", {
      session_id: `corrupt-${name}`,
      session_dir: corruptSessionDir
    });
    assert(!response.ok, `${name} browser state should be rejected`);
    assert(response.output.includes(expectedError), `${name} should report ${expectedError}`);
    if (stateIsDirectory) {
      assert(fs.statSync(corruptStatePath).isDirectory(), `${name} state should remain untouched`);
    } else {
      assert(
        fs.readFileSync(corruptStatePath, "utf8") === originalState,
        `${name} state should not be overwritten`
      );
    }
  } finally {
    fs.rmSync(corruptSessionDir, { recursive: true, force: true });
  }
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
  await assertCorruptStateRejected(
    "invalid-json",
    (statePath) => fs.writeFileSync(statePath, '{"schema":\n', { mode: 0o600 }),
    "contains invalid JSON"
  );
  await assertCorruptStateRejected(
    "invalid-shape",
    (statePath) =>
      fs.writeFileSync(
        statePath,
        `${JSON.stringify({ schema: "cindx.browser-session.v1", session_id: "partial" })}\n`,
        { mode: 0o600 }
      ),
    "has invalid shape"
  );
  await assertCorruptStateRejected(
    "unreadable",
    (statePath) => fs.mkdirSync(statePath),
    "is unreadable"
  );
  const corruptCleanupSessionDir = path.join(root, "corrupt-cleanup");
  const corruptCleanupStatePath = path.join(corruptCleanupSessionDir, "session-state.json");
  const cleanupProbeSessionDir = path.join(root, "cleanup-probe");
  const corruptCleanupState = '{"schema":\n';
  fs.mkdirSync(corruptCleanupSessionDir, { recursive: true });
  fs.writeFileSync(corruptCleanupStatePath, corruptCleanupState, { mode: 0o600 });
  const isolatedCleanup = await invokeRaw("close", {
    session_id: "cleanup-probe",
    session_dir: cleanupProbeSessionDir
  });
  assert(
    isolatedCleanup.ok && isolatedCleanup.output === "browser session already closed",
    "a corrupt sibling must not block a healthy browser session"
  );
  assert(
    fs.readFileSync(corruptCleanupStatePath, "utf8") === corruptCleanupState,
    "cleanup must not overwrite corrupt sibling browser state"
  );
  fs.rmSync(corruptCleanupSessionDir, { recursive: true, force: true });
  fs.rmSync(cleanupProbeSessionDir, { recursive: true, force: true });

  const deletedWatchdogStatePath = path.join(root, "deleted-watchdog-state.json");
  const deletedWatchdog = spawnSync(
    process.execPath,
    [sidecar, "--watch-session", deletedWatchdogStatePath, "deleted-launch"],
    { cwd: repoRoot, encoding: "utf8" }
  );
  assert(
    deletedWatchdog.status === 0,
    `watchdog should exit cleanly after normal state deletion: ${deletedWatchdog.stderr}`
  );
  const corruptWatchdogStatePath = path.join(root, "corrupt-watchdog-state.json");
  const corruptWatchdogState = '{"schema":\n';
  fs.writeFileSync(corruptWatchdogStatePath, corruptWatchdogState, { mode: 0o600 });
  const corruptWatchdog = spawnSync(
    process.execPath,
    [sidecar, "--watch-session", corruptWatchdogStatePath, "corrupt-launch"],
    { cwd: repoRoot, encoding: "utf8" }
  );
  assert(corruptWatchdog.status !== 0, "watchdog should reject corrupt browser state");
  assert(
    corruptWatchdog.stderr.includes("contains invalid JSON"),
    "watchdog should report corrupt browser state"
  );
  assert(
    fs.readFileSync(corruptWatchdogStatePath, "utf8") === corruptWatchdogState,
    "watchdog must not overwrite corrupt browser state"
  );

  const opened = await invoke("open", { url: baseUrl });
  assert(opened.page?.id, "open should return a CDP target id");
  assert(!fs.existsSync(abandonedLockPath), "an abandoned partial lock should be reclaimed");
  assert(!fs.existsSync(staleStatePath), "the oldest expired sibling session should be cleaned");
  const statePath = path.join(sessionDir, "session-state.json");
  const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
  const browserPid = state.browser_pid;
  const watchdogPid = state.watchdog_pid;
  assert(processIsAlive(browserPid), "open should leave a live browser process");
  assert(processIsAlive(watchdogPid), "open should supervise the browser with a watchdog");
  assert(state.schema === "cindx.browser-session.v1", "browser state should be versioned");
  assert(state.launch_token, "browser state should identify the Cindx-owned launch");
  assert(
    state.lease_expires_at_ms > state.last_used_at_ms,
    "browser state should carry a renewable inactivity lease"
  );

  spawnSync("kill", [String(watchdogPid)]);
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert(!processIsAlive(watchdogPid), "legacy migration fixture should not retain a watchdog");
  const initialState = JSON.parse(fs.readFileSync(statePath, "utf8"));
  fs.writeFileSync(
    statePath,
    `${JSON.stringify({
      browser_pid: initialState.browser_pid,
      executable: initialState.executable,
      active_tab_id: initialState.active_tab_id
    })}\n`,
    { mode: 0o600 }
  );
  await invoke("tabs");
  const migratedState = JSON.parse(fs.readFileSync(statePath, "utf8"));
  const migratedWatchdogPid = migratedState.watchdog_pid;
  assert(
    migratedState.schema === "cindx.browser-session.v1" &&
      migratedState.watchdog_pid &&
      migratedState.profile_dir === path.join(sessionDir, "profile"),
    "a live legacy session should migrate to a complete path-bound v1 state"
  );

  const mismatchedState = {
    ...migratedState,
    profile_dir: path.join(root, "different-session", "profile")
  };
  fs.writeFileSync(statePath, `${JSON.stringify(mismatchedState)}\n`, { mode: 0o600 });
  const rejectedMismatchedProfile = await invokeRaw("tabs");
  assert(!rejectedMismatchedProfile.ok, "a mismatched browser profile should be rejected");
  assert(
    rejectedMismatchedProfile.output.includes("has invalid shape"),
    "a mismatched browser profile should report invalid state ownership"
  );
  assert(
    fs.readFileSync(statePath, "utf8") === `${JSON.stringify(mismatchedState)}\n`,
    "a mismatched browser profile must remain untouched"
  );
  fs.writeFileSync(statePath, `${JSON.stringify(migratedState)}\n`, { mode: 0o600 });

  const validState = fs.readFileSync(statePath, "utf8");
  const corruptedState = '{"schema":\n';
  fs.writeFileSync(statePath, corruptedState, { mode: 0o600 });
  const rejectedAction = await invokeRaw("extract_text", { selector: "body" });
  assert(!rejectedAction.ok, "an action should reject corrupt live browser state");
  assert(
    rejectedAction.output.includes("contains invalid JSON"),
    "an action should report corrupt live browser state"
  );
  assert(
    fs.readFileSync(statePath, "utf8") === corruptedState,
    "an action must not overwrite corrupt browser state with partial metadata"
  );
  fs.writeFileSync(statePath, validState, { mode: 0o600 });

  await invoke("type", { role: "textbox", name: "Message", text: "hello-cdp" });
  await invoke("click", { role: "button", name: "Increment", wait_for: "#count" });
  const text = await invoke("extract_text", { selector: "body" });
  assert(text.output.includes("hello-cdp"), "typed text should persist across sidecar processes");
  assert(text.output.includes("1"), "click should update the page");

  const [concurrentTabs, concurrentText] = await Promise.all([
    invoke("tabs"),
    invoke("extract_text", { selector: "body" })
  ]);
  assert(concurrentTabs.tabs.length >= 1, "concurrent tab inspection should be serialized safely");
  assert(concurrentText.output.includes("hello-cdp"), "concurrent text inspection should succeed");
  assert(
    !fs.existsSync(path.join(sessionDir, ".action-lock.json")),
    "browser action lock should be released after each request"
  );

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
  await new Promise((resolve) => setTimeout(resolve, 100));
  assert(!processIsAlive(migratedWatchdogPid), "close should terminate the session watchdog");
  const closedAgain = await invoke("close");
  assert(
    closedAgain.output === "browser session already closed",
    "closing an inactive session must be idempotent without relaunching a browser"
  );
  assert(!fs.existsSync(statePath), "idempotent close must not recreate browser state");
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
