# Cindx Test Builds

Download current builds from [GitHub Releases](https://github.com/DaleXiao/Cindx/releases).

Current source release: [0.1.94](0.1.94.md).

Build archives belong in GitHub Actions or GitHub Releases and must not be
committed to this directory.

## Runtime data and startup logs

Installed builds store state under:

```text
~/Library/Application Support/Cindx
```

If Cindx does not open, inspect `startup.log` in that directory. The application
requires persistent SQLite state. If that database cannot be opened safely,
Cindx aborts startup and records `persistent state unavailable; startup aborted`
instead of silently running with an in-memory substitute.
