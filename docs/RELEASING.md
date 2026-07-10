# Releasing Cindx

## Continuous integration

Every push to `main` and every pull request runs the Rust, Tauri bridge,
frontend, and desktop structure checks. Pushes to `main` also produce an
Apple Silicon test ZIP in the workflow run. The artifact is retained for seven
days and is not committed to Git history.

The packaged binary is also run in startup-probe mode with a clean temporary
Home. This verifies that first launch does not depend on a build-machine path and
that the user-scoped SQLite state can be created before an artifact is uploaded.

## Tagged releases

A tag matching the committed application version starts the Release workflow.
For example, version `0.0.5` must be committed before pushing tag `v0.0.5`.

```sh
node scripts/check-release-version.mjs v0.0.5
git tag v0.0.5
git push origin v0.0.5
```

The workflow builds `universal-apple-darwin`, creates a prerelease named
`Cindx v0.0.5`, and uploads the archived `.app` bundle to GitHub Releases. The
Universal application runs on Apple Silicon and Intel Macs. DMG generation is
deliberately disabled because it adds a separate macOS scripting failure point
without improving internal testing.

The same release can be started from Actions > Release > Run workflow by
entering a tag that matches the committed version. The workflow creates the tag
at the selected commit when it does not already exist.

CI invokes Tauri directly and never increments source versions. Local
`npm run tauri -- build` still uses `scripts/run-tauri.mjs` and increments the
patch version before a successful local build.

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
