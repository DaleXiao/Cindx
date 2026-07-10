# Cindx Test Builds

Download current builds from [GitHub Releases](https://github.com/DaleXiao/Cindx/releases).

Version `0.0.4` embedded its build-machine workspace path and can exit during
first launch on another Mac. It is retained only as a historical bootstrap
artifact and must not be used for testing. Use `0.0.5` or newer.

## 0.0.4 macOS arm64

`Cindx_0.0.4_aarch64.zip` is for Apple Silicon Macs (M1 or newer).

SHA-256:

```text
833f5ca0e44d98ae429a9413aa79473e7687661c4b0c992882cea10c026cccdd
```

Historical install notes:

1. Download and unzip the package.
2. Move `Cindx.app` to `/Applications`.
3. Open it from Finder.

This internal build is ad-hoc signed but not Apple-notarized. If macOS blocks
the first launch, open System Settings > Privacy & Security and choose Open
Anyway. You can also right-click `Cindx.app` in Finder and choose Open.

Do not use this package on an Intel Mac. A separate `x86_64` package is required.

## Runtime data and startup logs

Installed builds store state under:

```text
~/Library/Application Support/Cindx
```

If Cindx does not open, inspect `startup.log` in that directory. The application
falls back to in-memory state when its persistent SQLite database cannot be
opened, so a storage error should no longer terminate the process.
