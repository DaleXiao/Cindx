#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

const REQUEST_SCHEMA = "cindx.browser-control.v2";
const RESPONSE_SCHEMA = "cindx.browser-control-result.v2";
const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_OUTPUT_CHARS = 12_000;

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
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
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
  writeJson(statePath, { browser_pid: child.pid, executable, active_tab_id: null });

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const endpoint = endpointFromProfile(profileDir);
    if (await endpointIsAlive(endpoint)) return endpoint;
    await sleep(50);
  }
  throw new Error(`browser CDP endpoint did not start within ${timeoutMs}ms`);
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
  if (!(await endpointIsAlive(endpoint))) {
    debug("launching browser");
    endpoint = await launchBrowser(request, profileDir, statePath, timeoutMs);
  }
  debug(`connecting CDP ${endpoint}`);
  const { chromium } = playwrightCore().api;
  const browser = await chromium.connectOverCDP(endpoint, { timeout: timeoutMs });
  debug("CDP connected");
  const context = browser.contexts()[0];
  if (!context) throw new Error("CDP browser did not expose a default context");
  return { browser, context, endpoint, statePath, timeoutMs };
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

function sessionState(statePath) {
  try {
    return readJson(statePath);
  } catch {
    return {};
  }
}

function saveActiveTab(statePath, activeTabId) {
  const state = sessionState(statePath);
  writeJson(statePath, { ...state, active_tab_id: activeTabId });
}

async function selectPage(context, request, statePath) {
  let pages = await pagesWithIds(context);
  if (pages.length === 0) {
    const page = await context.newPage();
    pages = [{ page, index: 0, id: await pageId(context, page) }];
  }
  const requestedId = request.tab_id || sessionState(statePath).active_tab_id;
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
    await runtime.browser.close();
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
  try {
    debug(`execute ${request.id} begin`);
    if (request.schema !== REQUEST_SCHEMA) throw new Error(`unsupported schema ${request.schema}`);
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
  } catch (caught) {
    error = caught instanceof Error ? caught : new Error(String(caught));
  }

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
  const response = {
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
  return response;
}

async function main() {
  if (process.argv.includes("--health")) {
    health();
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
