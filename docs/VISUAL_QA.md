# Cindx Desktop Visual QA

Validated on macOS against the packaged Tauri application.

## Product Criteria

- Desktop-first local agent workspace, not a dashboard or landing page.
- Project-to-session navigation on the left, one session thread in the center.
- Selection-driven Inspector on the right for details, artifacts, and context.
- Trace remains an independent workspace view.
- Grayscale interface with compact typography and icons.
- Advanced runtime, model, knowledge, tool, and permission controls live in Settings.

## Viewport Checks

| Scenario | Window | Result | Evidence |
| --- | --- | --- | --- |
| Session default | 1280 x 820 | Inspector visible; thread retains the primary reading width | [session-default.jpg](visual-qa/session-default.jpg) |
| Runtime settings | 1280 x 820 | Inspector hidden; controls follow the settings hierarchy | [settings-runtime.jpg](visual-qa/settings-runtime.jpg) |
| Trace wide | 1532 x 768 | Trace has a dedicated workspace and Details Inspector | [trace-wide.jpg](visual-qa/trace-wide.jpg) |
| Session compact | 1024 x 768 | Inspector starts closed; composer and thread remain usable | [session-compact-1024.jpg](visual-qa/session-compact-1024.jpg) |
| Compact Inspector | 1024 x 768 | Inspector overlays the workspace instead of shrinking it | [session-compact-inspector.jpg](visual-qa/session-compact-inspector.jpg) |

## Issues Found And Fixed

- Replaced duplicate Timeline, Conversation, and Agent Output surfaces with one Session Thread.
- Removed colored status decoration and consolidated the interface into one grayscale token system.
- Fixed empty grid cells in model roles and RAG statistics.
- Fixed Browser and Computer sidecar startup in packaged GUI launches by resolving Node outside the shell PATH.
- Made state, trace, retry, cancellation, permission resume, and export session-aware.
- Preserved full session history across multiple agent runs.

## Automated Verification

- Desktop structure and grayscale guards.
- Layout contracts at 1440, 1280, and 1024 widths.
- Frontend production build and strict TypeScript checks.
- Desktop Rust tests and full Cargo workspace tests.
- Signed macOS application bundle verified with `codesign --verify --deep --strict`.

Final application: `apps/desktop/src-tauri/target/release/bundle/macos/Cindx.app`
