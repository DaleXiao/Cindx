# Releasing Cindx

## Continuous integration

Every push to `main` and every pull request runs the documentation baseline,
Rust, Tauri bridge, frontend, and desktop structure checks. Pushes to `main` also produce an
Apple Silicon test ZIP in the workflow run. The artifact is retained for seven
days and is not committed to Git history.

The packaged binary is also run in startup-probe mode with a clean temporary
Home. This verifies that first launch does not depend on a build-machine path and
that the user-scoped SQLite state can be created before an artifact is uploaded.

## Tagged releases

A tag matching the committed application version starts the Release workflow.
Read the version from the committed Tauri configuration before creating the tag.

```sh
VERSION="$(node -p "require('./apps/desktop/src-tauri/tauri.conf.json').version")"
node scripts/check-release-version.mjs "v$VERSION"
git tag "v$VERSION"
git push origin "v$VERSION"
```

The workflow builds `universal-apple-darwin`, creates a matching prerelease, and
uploads the archived `.app` bundle to GitHub Releases. The
Universal application runs on Apple Silicon and Intel Macs. DMG generation is
deliberately disabled because it adds a separate macOS scripting failure point
without improving internal testing.

The same release can be started from Actions > Release > Run workflow by
entering a tag that matches the committed version. The workflow creates the tag
at the selected commit when it does not already exist.

CI invokes Tauri directly and never increments source versions. Local
`npm run tauri -- build` still uses `scripts/run-tauri.mjs` and increments the
patch version before a successful local build.

Before tagging, `docs/CURRENT.md` must carry the same application version and
`node scripts/check-docs.mjs` must pass. Provider-backed evaluation is not run
implicitly by the release workflow; any quality claim needs a separately
versioned report under `docs/evaluations/`.

## Optional Apple signing and notarization

Unsigned test releases can be downloaded and opened through macOS Privacy &
Security. To make releases open normally for users, configure these repository
Actions secrets:

- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application `.p12`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_ID`
- `APPLE_PASSWORD`: app-specific Apple ID password
- `APPLE_TEAM_ID`

The workflow selects exactly one release path:

- With no certificate secrets, it builds an unsigned prerelease and does not set
  any Apple signing environment variables.
- With both certificate secrets, it imports the certificate into a temporary
  keychain and builds a signed prerelease.
- With the certificate and all three notarization secrets, it builds a signed
  and notarized prerelease.

Partial certificate or notarization configuration fails early with a clear
error. The certificate payload and password are scoped to the import step so
Tauri cannot attempt a second `security import`, and an absent signing identity
is never passed to Tauri as an empty string.
