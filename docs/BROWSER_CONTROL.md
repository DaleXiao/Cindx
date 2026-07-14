# Browser Control v2

Cindx browser control uses two layers with separate responsibilities:

- CDP owns browser process discovery, reusable sessions, target ids, tab focus,
  and network/page telemetry.
- Playwright owns semantic locators, frames, navigation waiting, downloads,
  screenshots, and accessibility snapshots.

Playwright is not a replacement for CDP here. `playwright-core` connects to the
dedicated Chromium process over CDP and supplies the higher-level interaction
semantics that raw CDP does not provide.

Computer Use remains a separate accessibility/native-control channel. Browser
selectors and DOM state must not be treated as desktop accessibility state.

## Runtime

The app bundles the pinned `playwright-core` client and the Node sidecar. The
controller uses the first available browser from:

1. `CINDX_CHROMIUM`
2. Google Chrome
3. Microsoft Edge
4. Chromium

Each Cindx session receives a dedicated profile under
`.cindx/browser-sessions/`. Browser state is therefore reusable within a
session without attaching to the user's normal browser profile.

## Protocol

Rust sends a private request file using schema `cindx.browser-control.v2` and
expects one JSON response using `cindx.browser-control-result.v2`.

Supported actions:

- `open`
- `extract_text`
- `capture`
- `click`
- `type`
- `scroll`
- `tabs`
- `select_tab`

Targets can use an accessible role and name, label, placeholder, visible text,
CSS selector, frame name/URL fragment, or coordinates where explicitly
supported. Tab ids are CDP target ids and remain stable across one browser
process lifetime.

## Safety And Observability

- Every browser tool goes through the existing Cindx permission boundary.
- Typed text is redacted from traces; only its character count is retained.
- Request files use owner-only permissions and are removed after execution.
- Session and artifact paths are constrained to the active workspace.
- Rust enforces cooperative cancellation and a bounded hard timeout around the
  sidecar process.
- Each action writes `cindx.browser-control-trace.v2` with duration, page,
  target, and CDP network counters.
- Screenshots, extracted text, and downloads return as normal tool artifacts.

## Verification

The integration fixture starts a local site and drives a real headless Chromium
session through open, semantic typing/clicking, iframe targeting, extraction,
screenshot capture, tab selection, and download handling:

```sh
node scripts/test-browser-sidecar.mjs
```

The same fixture is required by desktop checks, local release builds, and CI.
