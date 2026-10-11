# Pulse

An Activity-Monitor-style system monitor for Windows. It shows **applications, not a wall of
processes**: every helper process is folded into the app that started it, and each app gets one
CPU, memory, disk, network and GPU figure. Tray readouts, a drop-down panel and a full window
share the same live data, and a 30-day history says what happened while you were away.

简体中文界面与英文界面都有；语言跟随系统，也可以在设置里指定。

## What it does

- **Tray readouts** — one notification-area icon per reading (CPU, memory, disk, network, GPU,
  battery, temperature) as a value, a graph, or both. Click one to open the panel.
- **Panel** — tiles or a list of every reading, the busiest apps, and quit-from-here.
- **Window** — Overview, CPU, Memory, Disk, Network, GPU, Power/Temperature and Sound tabs; an
  application list with search, sort, pins and Quit / Force quit; history from one hour to 30 days.
- **Worth a look** — apps that keep a core busy, grow in memory over half an hour, hammer the disk
  or the network, plus a toast for each (de-duplicated, with a cool-down).
- **Sound mixer** — per-application volume and mute with live level meters, remembered across
  restarts.

Nothing is faked: a reading Windows does not give an unprivileged process is shown as unavailable
instead of as zero. Per-app network *bytes*, CPU package temperature and fan speed are not
available without a kernel driver or elevation; Pulse shows connections and ports instead and says so.

Pulse has no account, no telemetry and makes no network requests.

## Requirements

- Windows 10/11 x64 with the WebView2 runtime
- Rust, **stable-x86_64-pc-windows-gnu**, with MinGW binutils on `PATH` (`dlltool`, `windres`)
- Node 24 and pnpm 10

## Develop

```bash
pnpm install
pnpm tauri dev
```

Things worth knowing:

- A debug build loads the Vite dev server, so it never registers itself to start at sign-in, and it
  removes a sign-in entry that points at itself (an older build used to leave one behind, which
  opened an empty "localhost refused to connect" window at every sign-in).
- `PULSE_DEV_MIRROR=1 pnpm tauri dev` makes the debug backend mirror its state into
  `%APPDATA%\com.tzz.pulse\dev`, so `pnpm dev` in a plain browser shows the same live figures.
- If `cargo` fails once with `dlltool could not create import library … .dll_imports`, it is a
  parallel-build race in some MinGW distributions: re-run with `-j 2`.
- The UI probes in `scripts/*.mjs` talk to the WebView2 debugging port; each file's header says how
  to start the app for it (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=…`).

### Checks

```bash
pnpm build          # tsc --noEmit + vite build
pnpm test           # vitest (pure formatting, series, ordering, number parsing, …)
cd src-tauri && cargo test
node scripts/release-check.mjs   # runs the release binary and reads the CSP header off it
```

## Build and package

```bash
pnpm tauri build          # NSIS installer (per-user)
pnpm package:portable     # zip with pulse.exe and WebView2Loader.dll
```

## Where things live

User data is in `%APPDATA%\com.tzz.pulse\`:

| File | What |
|---|---|
| `settings.json` | versioned and migrated on load (the previous version is kept once as `settings.v<N>.json.bak`); an unreadable file is kept as `settings.json.bad` |
| `history.json` | one-minute points for two hours, five-minute for a day, hourly for 30 days; the busiest apps per hour for 48 hours |
| `insights.json` | the "worth a look" list |
| `audio-volumes.json` | remembered per-app volumes |
| `logs/pulse.log` | diagnostics (rotated at 512 KiB); the last lines are in Settings → About → Copy report |

Source layout:

- `src-tauri/src` — `metrics/` (sampler, process grouping, PDH counters, GPU, battery, network),
  `history`, `alerts`, `settings`, `audio` (Core Audio), `tray*` / `menu` (notification-area icons
  and the panel), `quit` (what may be closed and how), `toast`, `i18n`, `diag`, `ipc`.
- `src` — `App.tsx` (window), `panel/` (tray panel), `components/`, `lib/` (store, IPC, pure
  helpers with tests), `i18n/`.
