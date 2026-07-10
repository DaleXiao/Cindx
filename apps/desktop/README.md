# Desktop App Placeholder

This directory contains the Tauri desktop shell for the local agent.

Initial frontend target:

- React
- TypeScript
- Vite
- Tauri v2

Initial screens:

- Chat and task timeline.
- Tool-call details.
- Permission prompt.
- Settings.

## Commands

```sh
npm install
npm run build
npm run tauri dev
```

From the repository root, the IPv4-first dependency install helper is:

```sh
scripts/install-desktop-deps-ipv4.sh
```

The frontend also runs as a browser preview with:

```sh
npm run dev
```

From the repository root, the desktop verification command is:

```sh
scripts/check-desktop.sh
```
