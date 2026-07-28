#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { spawn, spawnSync } = require("child_process");

const REQUEST_SCHEMA = "cindx.browser-control.v2";
const RESPONSE_SCHEMA = "cindx.browser-control-result.v2";
const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_OUTPUT_CHARS = 12_000;
const SESSION_STATE_SCHEMA = "cindx.browser-session.v1";
const DEFAULT_SESSION_TTL_MS = 30 * 60 * 1_000;
const MIN_SESSION_TTL_MS = 5 * 60 * 1_000;
const MAX_SESSION_TTL_MS = 24 * 60 * 60 * 1_000;
const SESSION_WATCH_INTERVAL_MS = 15_000;
const SESSION_LOCK_NAME = ".action-lock.json";
const MALFORMED_LOCK_GRACE_MS = 5_000;
const MAX_STALE_SESSION_CLEANUPS_PER_ACTION = 128;

function debug(message) {
  if (process.env.CINDX_BROWSER_DEBUG === "1") {
    process.stderr.write(`[browser-sidecar] ${message}\n`);
  }
}

function playwrightCore() {
  const candidates = [
    process.env.CINDX_PLAYWRIGHT_CORE,
    path.join(__dirname, "node_modules", "playwright-core"),
    path.join(__dirname, "..", "..", "apps", "desktop", "node_modules", "playwright-core")
  ].filter(Boolean);
  const errors = [];
  for (const candidate of candidates) {
    try {
      return {
        api: require(candidate),
        version: require(path.join(candidate, "package.json")).version
      };
    } catch (error) {
      errors.push(`${candidate}: ${error.message}`);
    }
  }
  throw new Error(`playwright-core is unavailable\n${errors.join("\n")}`);
}

function browserExecutable() {
  const configured = process.env.CINDX_CHROMIUM;
  const candidates = [
    configured,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    process.platform === "win32"
      ? path.join(process.env.PROGRAMFILES || "", "Google", "Chrome", "Application", "chrome.exe")
      : null
  ].filter(Boolean);
  return candidates.find((candidate) => fs.existsSync(candidate));
}

function health() {
  const core = playwrightCore();
  const executable = browserExecutable();
  if (!executable) throw new Error("Chrome, Edge, or Chromium is required");
  process.stdout.write(`browser-sidecar v2 ok playwright=${core.version} browser=${executable}\n`);
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const temporaryPath = `${filePath}.${process.pid}.${crypto.randomUUID()}.tmp`;
  try {
    fs.writeFileSync(temporaryPath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
    fs.renameSync(temporaryPath, filePath);
  } finally {
    fs.rmSync(temporaryPath, { force: true });
  }
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function asBoolean(value, fallback = false) {
  if (value === undefined || value === null || value === "") return fallback;
  if (typeof value === "boolean") return value;
  return ["1", "true", "yes", "y"].includes(String(value).toLowerCase());
}

function asNumber(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function timeoutFor(request) {
  return Math.min(Math.max(asNumber(request.timeout_ms, DEFAULT_TIMEOUT_MS), 1_000), 120_000);
}

function sessionTtlMs() {
  return Math.min(
    Math.max(asNumber(process.env.CINDX_BROWSER_SESSION_TTL_MS, DEFAULT_SESSION_TTL_MS), MIN_SESSION_TTL_MS),
    MAX_SESSION_TTL_MS
  );
}

function truncate(value, maximum = MAX_OUTPUT_CHARS) {
  const text = String(value || "");
  return text.length <= maximum ? text : `${text.slice(0, maximum)}...`;
}

function safeFileName(value) {
  const cleaned = String(value || "download")
    .replace(/[^a-zA-Z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 120);
  return cleaned || "download";
}

function processCommand(pid) {
  if (!Number.isInteger(pid) || pid <= 0 || process.platform === "win32") return "";
  const result = spawnSync("ps", ["-p", String(pid), "-o", "command="], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"]
  });
  return result.status === 0 ? String(result.stdout || "").trim() : "";
}

function browserProcessBelongsToSession(state, profileDir) {
  const pid = Number(state.browser_pid);
  if (!processIsAlive(pid)) return false;
  if (state.profile_dir && path.resolve(state.profile_dir) !== path.resolve(profileDir)) return false;
  if (process.platform === "win32") {
    return Boolean(state.launch_token && state.profile_dir);
  }
  const command = processCommand(pid);
  return Boolean(command && command.includes(`--user-data-dir=${profileDir}`));
}

function watchdogProcessBelongsToSession(state, statePath) {
  const pid = Number(state.watchdog_pid);
  if (!processIsAlive(pid)) return false;
  if (process.platform === "win32") return Boolean(state.launch_token);
  const command = processCommand(pid);
  return Boolean(
    command &&
      command.includes(path.basename(__filename)) &&
      command.includes("--watch-session") &&
      command.includes(statePath) &&
      command.includes(String(state.launch_token || ""))
  );
}

function sessionLockMetadata(lockPath) {
  try {
    return readJson(lockPath);
  } catch {
    return {};
  }
}

function lockOwnerIsAlive(metadata) {
  const pid = Number(metadata.pid);
  if (!processIsAlive(pid)) return false;
  if (process.platform === "win32") return true;
  const command = processCommand(pid);
  return Boolean(command && command.includes(path.basename(__filename)));
}

function sessionLockAgeMs(lockPath, metadata) {
  const acquiredAtMs = asNumber(metadata.acquired_at_ms, 0);
  if (acquiredAtMs > 0) return Math.max(0, Date.now() - acquiredAtMs);
  try {
    return Math.max(0, Date.now() - fs.statSync(lockPath).mtimeMs);
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

async function acquireSessionLock(sessionDir, requestId, waitMs) {
  const lockPath = path.join(sessionDir, SESSION_LOCK_NAME);
  const deadline = Date.now() + Math.max(0, waitMs);
  const owner = {
    schema: "cindx.browser-session-lock.v1",
    pid: process.pid,
    request_id: String(requestId || "unknown"),
    acquired_at_ms: Date.now()
  };

  for (;;) {
    try {
      const handle = fs.openSync(lockPath, "wx", 0o600);
      try {
        fs.writeFileSync(handle, `${JSON.stringify(owner, null, 2)}\n`);
      } finally {
        fs.closeSync(handle);
      }
      return () => {
        const current = sessionLockMetadata(lockPath);
        if (current.pid === owner.pid && current.request_id === owner.request_id) {
          fs.rmSync(lockPath, { force: true });
        }
      };
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      const metadata = sessionLockMetadata(lockPath);
      const ageMs = sessionLockAgeMs(lockPath, metadata);
      const hasOwner = Number.isInteger(Number(metadata.pid)) && Number(metadata.pid) > 0;
      if (
        (hasOwner && !lockOwnerIsAlive(metadata)) ||
        (!hasOwner && ageMs > MALFORMED_LOCK_GRACE_MS) ||
        ageMs > 10 * 60 * 1_000
      ) {
        fs.rmSync(lockPath, { force: true });
        continue;
      }
      if (Date.now() >= deadline) {
        throw new Error(`browser session is busy with request ${metadata.request_id || "unknown"}`);
      }
      await sleep(50);
    }
  }
}

function endpointFromProfile(profileDir) {
  const portFile = path.join(profileDir, "DevToolsActivePort");
  if (!fs.existsSync(portFile)) return null;
  const [port] = fs.readFileSync(portFile, "utf8").trim().split(/\r?\n/);
  return /^\d+$/.test(port || "") ? `http://127.0.0.1:${port}` : null;
}

async function endpointIsAlive(endpoint) {
  if (!endpoint) return false;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 1_000);
  try {
    const response = await fetch(`${endpoint}/json/version`, { signal: controller.signal });
    return response.ok;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}

async function launchBrowser(request, profileDir, statePath, timeoutMs) {
  const executable = browserExecutable();
  if (!executable) throw new Error("Chrome, Edge, or Chromium is required for browser control");
  fs.mkdirSync(profileDir, { recursive: true });
  const portFile = path.join(profileDir, "DevToolsActivePort");
  fs.rmSync(portFile, { force: true });
  const args = [
    `--user-data-dir=${profileDir}`,
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    "--remote-allow-origins=*",
    "--no-first-run",
    "--no-default-browser-check",
    "about:blank"
  ];
  if (asBoolean(request.headless)) args.unshift("--headless=new", "--disable-gpu");
  const child = spawn(executable, args, { detached: true, stdio: "ignore" });
  child.unref();
  const launchToken = crypto.randomUUID();
  const createdAt = Date.now();
  writeJson(statePath, {
    schema: SESSION_STATE_SCHEMA,
    session_id: request.session_id,
    browser_pid: child.pid,
    watchdog_pid: null,
    executable,
    profile_dir: profileDir,
    launch_token: launchToken,
    active_tab_id: null,
    created_at_ms: createdAt,
    last_used_at_ms: createdAt,
    lease_expires_at_ms: createdAt + sessionTtlMs()
  });

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const endpoint = endpointFromProfile(profileDir);
    if (await endpointIsAlive(endpoint)) {
      ensureSessionWatchdog(statePath);
      return endpoint;
    }
    await sleep(50);
  }
  terminateProcessTree(child.pid, "SIGTERM");
  await sleep(100);
  terminateProcessTree(child.pid, "SIGKILL");
  fs.rmSync(statePath, { force: true });
  throw new Error(`browser CDP endpoint did not start within ${timeoutMs}ms`);
}

function processIsAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

function terminateProcessTree(pid, signal) {
  if (!Number.isInteger(pid) || pid <= 0) return;
  if (process.platform === "win32") {
    if (signal === "SIGKILL") {
      spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
    }
    return;
  }
  try {
    process.kill(-pid, signal);
  } catch {}
  try {
    process.kill(pid, signal);
  } catch {}
}

function renewSessionLease(statePath) {
  const state = sessionState(statePath, { allowMissing: false });
  if (!state.launch_token) return state;
  const now = Date.now();
  const renewed = {
    ...state,
    last_used_at_ms: now,
    lease_expires_at_ms: now + sessionTtlMs()
  };
  writeJson(statePath, renewed);
  return renewed;
}

function ensureSessionWatchdog(statePath) {
  const state = sessionState(statePath, { allowMissing: false });
  if (!state.launch_token) return;
  if (watchdogProcessBelongsToSession(state, statePath)) {
    renewSessionLease(statePath);
    return;
  }
  const child = spawn(
    process.execPath,
    [__filename, "--watch-session", statePath, String(state.launch_token)],
    { detached: true, stdio: "ignore" }
  );
  child.unref();
  const now = Date.now();
  writeJson(statePath, {
    ...state,
    watchdog_pid: child.pid,
    last_used_at_ms: now,
    lease_expires_at_ms: now + sessionTtlMs()
  });
}

function stopSessionWatchdog(state, statePath) {
  if (!watchdogProcessBelongsToSession(state, statePath)) return;
  terminateProcessTree(Number(state.watchdog_pid), "SIGTERM");
}

async function waitForBrowserExit(state, profileDir, endpoint, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const ownedProcessAlive = browserProcessBelongsToSession(state, profileDir);
    if (!ownedProcessAlive && !(await endpointIsAlive(endpoint))) return true;
    await sleep(50);
  }
  return false;
}

async function terminateOwnedBrowserSession(statePath, endpoint, gracefulMs = 2_000) {
  const state = sessionState(statePath, { allowMissing: false });
  const profileDir = state.profile_dir || path.join(path.dirname(statePath), "profile");
  const browserPid = Number(state.browser_pid);
  if (processIsAlive(browserPid) && !browserProcessBelongsToSession(state, profileDir)) {
    throw new Error(`refusing to terminate unverified browser process ${browserPid}`);
  }
  if (browserProcessBelongsToSession(state, profileDir)) {
    terminateProcessTree(browserPid, "SIGTERM");
  }
  if (!(await waitForBrowserExit(state, profileDir, endpoint, gracefulMs))) {
    if (!browserProcessBelongsToSession(state, profileDir)) {
      throw new Error(`browser process ${browserPid || "unknown"} ownership changed before forced close`);
    }
    terminateProcessTree(browserPid, "SIGKILL");
  }
  if (!(await waitForBrowserExit(state, profileDir, endpoint, 1_000))) {
    throw new Error(`browser process ${browserPid || "unknown"} did not stop`);
  }
}

async function watchBrowserSession(statePath, launchToken) {
  const sessionDir = path.dirname(statePath);
  for (;;) {
    const state = sessionState(statePath, { allowMissing: true });
    if (!state.launch_token || state.launch_token !== launchToken) return;
    const expiresAt = asNumber(state.lease_expires_at_ms, 0);
    if (Date.now() < expiresAt) {
      await sleep(Math.min(SESSION_WATCH_INTERVAL_MS, Math.max(1_000, expiresAt - Date.now())));
      continue;
    }
    let release = null;
    try {
      release = await acquireSessionLock(
        sessionDir,
        `watchdog-${process.pid}-${launchToken}`,
        0
      );
    } catch {
      await sleep(SESSION_WATCH_INTERVAL_MS);
      continue;
    }
    let retry = false;
    try {
      const current = sessionState(statePath, { allowMissing: true });
      if (!current.launch_token || current.launch_token !== launchToken) return;
      if (!sessionLeaseExpired(current, statePath)) continue;
      const profileDir = current.profile_dir || path.join(sessionDir, "profile");
      const endpoint = endpointFromProfile(profileDir);
      await terminateOwnedBrowserSession(statePath, endpoint);
      const latest = sessionState(statePath, { allowMissing: true });
      if (latest.launch_token === launchToken) {
        fs.rmSync(path.join(profileDir, "DevToolsActivePort"), { force: true });
        fs.rmSync(statePath, { force: true });
      }
    } catch (error) {
      debug(`watchdog cleanup skipped: ${error.message}`);
      retry = true;
    } finally {
      release?.();
    }
    if (retry) {
      await sleep(SESSION_WATCH_INTERVAL_MS);
      continue;
    }
    return;
  }
}

function sessionLeaseExpired(state, statePath) {
  const explicitExpiry = asNumber(state.lease_expires_at_ms, 0);
  if (explicitExpiry > 0) return Date.now() >= explicitExpiry;
  try {
    return Date.now() >= fs.statSync(statePath).mtimeMs + sessionTtlMs();
  } catch {
    return false;
  }
}

async function cleanupExpiredSession(sessionDir) {
  const statePath = path.join(sessionDir, "session-state.json");
  let state = sessionState(statePath, { allowMissing: true });
  if (!state.browser_pid || !sessionLeaseExpired(state, statePath)) return;
  let release = null;
  try {
    release = await acquireSessionLock(sessionDir, `cleanup-${process.pid}`, 0);
  } catch {
    return;
  }
  try {
    state = sessionState(statePath, { allowMissing: true });
    if (!state.browser_pid || !sessionLeaseExpired(state, statePath)) return;
    const profileDir = state.profile_dir || path.join(sessionDir, "profile");
    const endpoint = endpointFromProfile(profileDir);
    if (processIsAlive(Number(state.browser_pid)) || (await endpointIsAlive(endpoint))) {
      await terminateOwnedBrowserSession(statePath, endpoint);
    }
    stopSessionWatchdog(state, statePath);
    fs.rmSync(path.join(profileDir, "DevToolsActivePort"), { force: true });
    fs.rmSync(statePath, { force: true });
  } catch (error) {
    debug(`expired session cleanup skipped for ${sessionDir}: ${error.message}`);
  } finally {
    release?.();
  }
}

async function cleanupExpiredSiblingSessions(activeSessionDir) {
  const sessionsRoot = path.dirname(activeSessionDir);
  let entries = [];
  try {
    entries = fs.readdirSync(sessionsRoot, { withFileTypes: true });
  } catch {
    return;
  }
  const candidates = entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(sessionsRoot, entry.name))
    .filter((candidate) => path.resolve(candidate) !== path.resolve(activeSessionDir))
    .map((candidate) => {
      try {
        return {
          candidate,
          stateModifiedAtMs: fs.statSync(path.join(candidate, "session-state.json")).mtimeMs
        };
      } catch {
        return { candidate, stateModifiedAtMs: Number.POSITIVE_INFINITY };
      }
    })
    .sort((left, right) => left.stateModifiedAtMs - right.stateModifiedAtMs)
    .slice(0, MAX_STALE_SESSION_CLEANUPS_PER_ACTION);
  for (const { candidate } of candidates) {
    try {
      await cleanupExpiredSession(candidate);
    } catch (error) {
      debug(`expired sibling cleanup skipped for ${candidate}: ${error.message}`);
    }
  }
}

async function closeBrowserSession(runtime) {
  const state = sessionState(runtime.statePath, { allowMissing: false });
  await Promise.race([
    runtime.browser.close().catch(() => {}),
    sleep(1_000)
  ]);
  if (processIsAlive(Number(state.browser_pid)) || (await endpointIsAlive(runtime.endpoint))) {
    await terminateOwnedBrowserSession(runtime.statePath, runtime.endpoint);
  }
  const sessionDir = path.dirname(runtime.statePath);
  fs.rmSync(path.join(sessionDir, "profile", "DevToolsActivePort"), { force: true });
  stopSessionWatchdog(state, runtime.statePath);
  fs.rmSync(runtime.statePath, { force: true });
}

async function connectBrowser(request) {
  const timeoutMs = timeoutFor(request);
  const sessionDir = request.session_dir;
  if (!sessionDir || !path.isAbsolute(sessionDir)) {
    throw new Error("session_dir must be an absolute path supplied by Cindx");
  }
  const profileDir = path.join(sessionDir, "profile");
  const statePath = path.join(sessionDir, "session-state.json");
  fs.mkdirSync(sessionDir, { recursive: true });
  let endpoint = endpointFromProfile(profileDir);
  let state = sessionState(statePath, { allowMissing: true });
  if (await endpointIsAlive(endpoint)) {
    if (state.session_id && state.session_id !== request.session_id) {
      throw new Error("browser session state belongs to a different Cindx session");
    }
    if (!browserProcessBelongsToSession(state, profileDir)) {
      throw new Error("live browser session ownership could not be verified");
    }
    if (!state.launch_token) {
      const now = Date.now();
      state = {
        ...state,
        schema: SESSION_STATE_SCHEMA,
        session_id: request.session_id,
        watchdog_pid: null,
        profile_dir: profileDir,
        launch_token: crypto.randomUUID(),
        active_tab_id: state.active_tab_id ?? null,
        created_at_ms: now,
        last_used_at_ms: now,
        lease_expires_at_ms: now + sessionTtlMs()
      };
      writeJson(statePath, state);
    }
    ensureSessionWatchdog(statePath);
  } else {
    if (browserProcessBelongsToSession(state, profileDir)) {
      await terminateOwnedBrowserSession(statePath, endpoint);
    }
    stopSessionWatchdog(state, statePath);
    fs.rmSync(path.join(profileDir, "DevToolsActivePort"), { force: true });
    fs.rmSync(statePath, { force: true });
    debug("launching browser");
    endpoint = await launchBrowser(request, profileDir, statePath, timeoutMs);
  }
  renewSessionLease(statePath);
  debug(`connecting CDP ${endpoint}`);
  const { chromium } = playwrightCore().api;
  const browser = await chromium.connectOverCDP(endpoint, { timeout: timeoutMs });
  debug("CDP connected");
  const context = browser.contexts()[0];
  if (!context) throw new Error("CDP browser did not expose a default context");
  return { browser, context, endpoint, statePath, profileDir, timeoutMs };
}

async function pageId(context, page) {
  const session = await context.newCDPSession(page);
  try {
    const result = await session.send("Target.getTargetInfo");
    return result.targetInfo.targetId;
  } finally {
    await session.detach().catch(() => {});
  }
}

async function pagesWithIds(context) {
  return Promise.all(
    context.pages().map(async (page, index) => ({ page, index, id: await pageId(context, page) }))
  );
}

function validProcessId(value) {
  return Number.isInteger(value) && value > 0;
}

function validOptionalTabId(value) {
  return value === null || value === undefined || typeof value === "string";
}

function validateSessionState(state, statePath) {
  if (!state || typeof state !== "object" || Array.isArray(state)) {
    throw new Error(`browser session state has invalid shape at ${statePath}: expected an object`);
  }

  if (state.schema === undefined) {
    if (
      !validProcessId(state.browser_pid) ||
      typeof state.executable !== "string" ||
      state.executable.length === 0 ||
      !validOptionalTabId(state.active_tab_id)
    ) {
      throw new Error(
        `browser session state has invalid shape at ${statePath}: incomplete legacy state`
      );
    }
    return {
      browser_pid: state.browser_pid,
      executable: state.executable,
      active_tab_id: state.active_tab_id ?? null
    };
  }

  const expectedProfileDir = path.resolve(path.dirname(statePath), "profile");
  const validWatchdogPid = state.watchdog_pid === null || validProcessId(state.watchdog_pid);
  const validActiveTabId = state.active_tab_id === null || typeof state.active_tab_id === "string";
  const validTimestamp = (value) => Number.isInteger(value) && value > 0;
  if (
    state.schema !== SESSION_STATE_SCHEMA ||
    typeof state.session_id !== "string" ||
    state.session_id.length === 0 ||
    !validProcessId(state.browser_pid) ||
    !validWatchdogPid ||
    typeof state.executable !== "string" ||
    state.executable.length === 0 ||
    typeof state.profile_dir !== "string" ||
    !path.isAbsolute(state.profile_dir) ||
    path.resolve(state.profile_dir) !== expectedProfileDir ||
    typeof state.launch_token !== "string" ||
    state.launch_token.length === 0 ||
    !validActiveTabId ||
    !validTimestamp(state.created_at_ms) ||
    !validTimestamp(state.last_used_at_ms) ||
    !validTimestamp(state.lease_expires_at_ms)
  ) {
    throw new Error(
      `browser session state has invalid shape at ${statePath}: expected ${SESSION_STATE_SCHEMA}`
    );
  }
  return state;
}

function sessionState(statePath, { allowMissing = false } = {}) {
  let source;
  try {
    source = fs.readFileSync(statePath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") {
      if (allowMissing) return {};
      throw new Error(`browser session state is missing at ${statePath}`);
    }
    throw new Error(
      `browser session state is unreadable at ${statePath}: ${error?.code || error?.message || error}`
    );
  }

  let state;
  try {
    state = JSON.parse(source);
  } catch (error) {
    throw new Error(
      `browser session state contains invalid JSON at ${statePath}: ${error.message}`
    );
  }
  return validateSessionState(state, statePath);
}

function saveActiveTab(statePath, activeTabId) {
  const state = sessionState(statePath, { allowMissing: false });
  const now = Date.now();
  writeJson(statePath, {
    ...state,
    active_tab_id: activeTabId,
    last_used_at_ms: now,
    lease_expires_at_ms: now + sessionTtlMs()
  });
}

async function selectPage(context, request, statePath) {
  let pages = await pagesWithIds(context);
  if (pages.length === 0) {
    const page = await context.newPage();
    pages = [{ page, index: 0, id: await pageId(context, page) }];
  }
  const requestedId =
    request.tab_id || sessionState(statePath, { allowMissing: false }).active_tab_id;
  const selected = pages.find((entry) => entry.id === requestedId) || pages[pages.length - 1];
  saveActiveTab(statePath, selected.id);
  return selected;
}

function frameScope(page, request) {
  if (!request.frame) return page;
  const matches = page
    .frames()
    .filter((frame) => frame !== page.mainFrame())
    .filter((frame) => frame.name() === request.frame || frame.url().includes(request.frame));
  if (matches.length !== 1) {
    throw new Error(`frame '${request.frame}' matched ${matches.length} frames`);
  }
  return matches[0];
}

function locatorFor(scope, request, required = true) {
  let locator = null;
  if (request.role) {
    locator = scope.getByRole(request.role, {
      name: request.name || undefined,
      exact: asBoolean(request.exact)
    });
  } else if (request.label) {
    locator = scope.getByLabel(request.label, { exact: asBoolean(request.exact) });
  } else if (request.placeholder) {
    locator = scope.getByPlaceholder(request.placeholder, { exact: asBoolean(request.exact) });
  } else if (request.text_target) {
    locator = scope.getByText(request.text_target, { exact: asBoolean(request.exact) });
  } else if (request.selector) {
    locator = scope.locator(request.selector);
  }
  if (!locator && required) {
    throw new Error("a semantic target or CSS selector is required");
  }
  return locator;
}

async function navigate(page, request, timeoutMs) {
  if (!request.url) return;
  await page.goto(request.url, {
    timeout: timeoutMs,
    waitUntil: request.wait_until || "domcontentloaded"
  });
}

async function cdpTelemetry(context, page) {
  const session = await context.newCDPSession(page);
  const events = { requests: 0, responses: 0, failed: 0, status_counts: {} };
  await session.send("Network.enable");
  await session.send("Page.enable");
  session.on("Network.requestWillBeSent", () => {
    events.requests += 1;
  });
  session.on("Network.responseReceived", ({ response }) => {
    events.responses += 1;
    const status = String(Math.trunc(response.status));
    events.status_counts[status] = (events.status_counts[status] || 0) + 1;
  });
  session.on("Network.loadingFailed", () => {
    events.failed += 1;
  });
  return { session, events };
}

async function pageSummary(context, page) {
  return {
    id: await pageId(context, page),
    url: page.url(),
    title: await page.title().catch(() => "")
  };
}

function traceRequest(request) {
  const redacted = { ...request };
  if (Object.prototype.hasOwnProperty.call(redacted, "text")) {
    redacted.text_length = String(redacted.text).length;
    redacted.text = "[redacted]";
  }
  delete redacted.session_dir;
  delete redacted.output_dir;
  return redacted;
}

function writeTrace(request, trace) {
  const outputDir = request.output_dir;
  fs.mkdirSync(outputDir, { recursive: true });
  const tracePath = path.join(outputDir, `${safeFileName(request.id)}-trace.json`);
  writeJson(tracePath, trace);
  return tracePath;
}

async function runAction(request, runtime) {
  debug(`running action ${request.action}`);
  const { context, statePath, timeoutMs } = runtime;
  if (request.action === "close") {
    await closeBrowserSession(runtime);
    return { output: "browser session closed", page: null, artifacts: [] };
  }

  if (request.action === "tabs") {
    const tabs = await Promise.all(
      (await pagesWithIds(context)).map(async ({ page, index, id }) => ({
        id,
        index,
        url: page.url(),
        title: await page.title().catch(() => "")
      }))
    );
    return {
      output: tabs.map((tab) => `${tab.id}\t${tab.title}\t${tab.url}`).join("\n"),
      page: null,
      tabs,
      artifacts: []
    };
  }

  let selected = await selectPage(context, request, statePath);
  let page = selected.page;
  if (request.action === "select_tab") {
    const tabs = await pagesWithIds(context);
    selected = tabs.find((entry) => entry.id === request.tab_id);
    if (!selected) throw new Error(`unknown tab_id ${request.tab_id}`);
    page = selected.page;
    await page.bringToFront();
    saveActiveTab(statePath, selected.id);
    return { output: "selected browser tab", page, artifacts: [] };
  }

  if (request.action === "open" && asBoolean(request.new_tab)) {
    page = await context.newPage();
  }
  page.setDefaultTimeout(timeoutMs);
  page.setDefaultNavigationTimeout(timeoutMs);
  const scope = () => frameScope(page, request);
  const artifacts = [];
  let output = "";

  if (!["open", "extract_text", "capture"].includes(request.action)) {
    await navigate(page, request, timeoutMs);
  }

  if (request.action === "open") {
    debug(`navigating ${request.url}`);
    await navigate(page, request, timeoutMs);
    debug("navigation complete");
    output = `opened ${page.url()}`;
  } else if (request.action === "extract_text") {
    await navigate(page, request, timeoutMs);
    const target = locatorFor(scope(), request, false) || scope().locator("body");
    const text = truncate(await target.innerText({ timeout: timeoutMs }));
    const aria = truncate(await target.ariaSnapshot({ timeout: timeoutMs }));
    const textPath = path.join(request.output_dir, `${safeFileName(request.id)}.txt`);
    fs.mkdirSync(request.output_dir, { recursive: true });
    fs.writeFileSync(textPath, `${text}\n`, { mode: 0o600 });
    artifacts.push({ path: textPath, mime_type: "text/plain", title: "Browser text" });
    output = `${text}\n\nAccessibility snapshot:\n${aria}`;
  } else if (request.action === "capture") {
    await navigate(page, request, timeoutMs);
    fs.mkdirSync(request.output_dir, { recursive: true });
    const screenshotPath = path.join(request.output_dir, `${safeFileName(request.id)}.png`);
    const textPath = path.join(request.output_dir, `${safeFileName(request.id)}.txt`);
    await page.screenshot({
      path: screenshotPath,
      fullPage: asBoolean(request.full_page, true),
      timeout: timeoutMs
    });
    const text = truncate(await page.locator("body").innerText({ timeout: timeoutMs }));
    fs.writeFileSync(textPath, `${text}\n`, { mode: 0o600 });
    artifacts.push(
      { path: screenshotPath, mime_type: "image/png", title: "Browser capture" },
      { path: textPath, mime_type: "text/plain", title: "Browser text" }
    );
    output = `captured ${page.url()}\n${truncate(text, 4_000)}`;
  } else if (request.action === "click") {
    const beforePages = new Set(context.pages());
    const locator = locatorFor(scope(), request, false);
    const click = () =>
      locator
        ? locator.click({ timeout: timeoutMs })
        : page.mouse.click(asNumber(request.x, NaN), asNumber(request.y, NaN));
    if (asBoolean(request.download)) {
      const [download] = await Promise.all([
        page.waitForEvent("download", { timeout: timeoutMs }),
        click()
      ]);
      const downloadPath = path.join(request.output_dir, safeFileName(download.suggestedFilename()));
      fs.mkdirSync(request.output_dir, { recursive: true });
      await download.saveAs(downloadPath);
      artifacts.push({ path: downloadPath, mime_type: null, title: "Browser download" });
      output = `downloaded ${download.suggestedFilename()}`;
    } else {
      await click();
      output = "clicked browser target";
    }
    const popup = context.pages().find((candidate) => !beforePages.has(candidate));
    if (popup) page = popup;
  } else if (request.action === "type") {
    const locator = locatorFor(scope(), request);
    if (asBoolean(request.append)) {
      await locator.pressSequentially(String(request.text || ""), { timeout: timeoutMs });
    } else {
      await locator.fill(String(request.text || ""), { timeout: timeoutMs });
    }
    if (asBoolean(request.press_enter)) await locator.press("Enter", { timeout: timeoutMs });
    output = `typed ${String(request.text || "").length} characters`;
  } else if (request.action === "scroll") {
    const x = asNumber(request.delta_x, 0);
    const y = asNumber(request.delta_y, 600);
    const locator = locatorFor(scope(), request, false);
    if (locator) {
      await locator.evaluate((element, delta) => element.scrollBy(delta.x, delta.y), { x, y });
    } else {
      await page.mouse.wheel(x, y);
    }
    output = `scrolled ${x},${y}`;
  } else {
    throw new Error(`unsupported browser action ${request.action}`);
  }

  if (request.wait_for) {
    await scope().locator(request.wait_for).waitFor({ state: "visible", timeout: timeoutMs });
  }
  await page.bringToFront();
  const activeId = await pageId(context, page);
  saveActiveTab(statePath, activeId);
  return { output, page, artifacts };
}

async function execute(request) {
  const startedAt = Date.now();
  fs.mkdirSync(request.output_dir, { recursive: true });
  let runtime = null;
  let telemetry = null;
  let actionResult = null;
  let error = null;
  let releaseSessionLock = null;
  try {
    debug(`execute ${request.id} begin`);
    if (request.schema !== REQUEST_SCHEMA) throw new Error(`unsupported schema ${request.schema}`);
    const sessionDir = request.session_dir;
    if (!sessionDir || !path.isAbsolute(sessionDir)) {
      throw new Error("session_dir must be an absolute path supplied by Cindx");
    }
    fs.mkdirSync(sessionDir, { recursive: true });
    await cleanupExpiredSiblingSessions(sessionDir);
    releaseSessionLock = await acquireSessionLock(
      sessionDir,
      request.id,
      timeoutFor(request) + 5_000
    );
    if (request.action === "close") {
      const profileDir = path.join(sessionDir, "profile");
      const statePath = path.join(sessionDir, "session-state.json");
      const endpoint = endpointFromProfile(profileDir);
      if (!(await endpointIsAlive(endpoint))) {
        const state = sessionState(statePath, { allowMissing: true });
        if (processIsAlive(Number(state.browser_pid))) {
          await terminateOwnedBrowserSession(statePath, endpoint);
        }
        stopSessionWatchdog(state, statePath);
        fs.rmSync(path.join(profileDir, "DevToolsActivePort"), { force: true });
        fs.rmSync(statePath, { force: true });
        actionResult = { output: "browser session already closed", page: null, artifacts: [] };
      }
    }
    if (actionResult) {
      debug("browser close required no live runtime");
    } else {
      runtime = await connectBrowser(request);
      debug("selecting page");
      const selected = request.action === "tabs" || request.action === "close"
        ? null
        : await selectPage(runtime.context, request, runtime.statePath);
      debug("attaching CDP telemetry");
      telemetry = selected ? await cdpTelemetry(runtime.context, selected.page) : null;
      debug("dispatching action");
      actionResult = await runAction(request, runtime);
      debug("action complete");
    }
  } catch (caught) {
    error = caught instanceof Error ? caught : new Error(String(caught));
  }

  try {
    const page = actionResult?.page || null;
    const pageInfo = page && runtime ? await pageSummary(runtime.context, page).catch(() => null) : null;
    if (telemetry) await telemetry.session.detach().catch(() => {});
    const trace = {
      schema: "cindx.browser-control-trace.v2",
      id: request.id,
      session_id: request.session_id,
      action: request.action,
      started_at_ms: startedAt,
      duration_ms: Date.now() - startedAt,
      request: traceRequest(request),
      cdp_endpoint: runtime?.endpoint || null,
      cdp_events: telemetry?.events || null,
      page: pageInfo,
      ok: !error,
      error: error?.message || null
    };
    const tracePath = writeTrace(request, trace);
    debug(`trace written ${tracePath}`);
    return {
      schema: RESPONSE_SCHEMA,
      ok: !error,
      id: request.id,
      action: request.action,
      session_id: request.session_id,
      controller: "cdp_playwright",
      page: pageInfo,
      tabs: actionResult?.tabs || null,
      output: error ? error.message : actionResult?.output || "browser action completed",
      artifacts: actionResult?.artifacts || [],
      trace_path: tracePath,
      duration_ms: trace.duration_ms,
      cdp_events: trace.cdp_events
    };
  } finally {
    releaseSessionLock?.();
  }
}

async function main() {
  if (process.argv.includes("--health")) {
    health();
    return;
  }
  if (process.argv[2] === "--watch-session") {
    const statePath = process.argv[3];
    const launchToken = process.argv[4];
    if (!statePath || !path.isAbsolute(statePath) || !launchToken) {
      throw new Error("browser session watchdog requires an absolute state path and launch token");
    }
    await watchBrowserSession(statePath, launchToken);
    return;
  }
  const requestPath = process.argv[2];
  if (!requestPath) throw new Error("missing request path");
  const request = readJson(requestPath);
  const response = await execute(request);
  debug("writing response");
  process.stdout.write(`${JSON.stringify(response)}\n`, () => process.exit(0));
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error.message || error}\n`);
  process.exit(1);
});
