# Cindx Test Builds

Download current builds from [GitHub Releases](https://github.com/DaleXiao/Cindx/releases).

Build archives belong in GitHub Actions or GitHub Releases and must not be
committed to this directory.

## Runtime data and startup logs

Installed builds store state under:

```text
~/Library/Application Support/Cindx
```

If Cindx does not open, inspect `startup.log` in that directory. The application
falls back to in-memory state when its persistent SQLite database cannot be
opened, so a storage error should no longer terminate the process.
