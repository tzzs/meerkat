# Meerkat

An Activity-Monitor-style system monitor for Windows. It shows **applications, not a wall of
processes**: every helper process is folded into the app that started it, and each app gets one
CPU, memory, disk, network and GPU figure. Tray readouts, a drop-down panel and a full window
share the same live data, and a 30-day history says what happened while you were away.

简体中文界面与英文界面都有；语言跟随系统，也可以在设置里指定。

## What it does

- **Tray readouts** — one notification-area icon per reading (CPU, memory, disk, network, GPU,
  battery, temperature) as a value, a graph, or both. Click one to open the panel.
- **Panel** — tiles or a list of every reading, the busiest apps, and quit-from-here.
- **Window** — Overview, History, CPU, Memory, Disk, Network, GPU, Power/Temperature, Sound and
  Bluetooth tabs; an application list with search, sort, pins and Quit / Force quit; history from
  one hour to 30 days.
- **Every app's own history** — what each one did, minute by minute, for 30 days. Click a row for
  the processes it is made of, their command lines, its windows, its ports, its services, and what
  it has read and written since it started. A History tab draws the machine on one timeline, and
  Windows' own event log — crashes, hangs, blue screens, driver resets, failed updates, what woke
  the machine — sits on the same axis.
- **How an app runs** — efficiency mode, priority, and pausing it, remembered per app and applied
  again to every process it starts. Paused apps are released when Meerkat exits.
- **Who wrote today's bytes, and what is listening on which port** — asked by name, with the
  project a dev server is serving.
- **Storage map** — a treemap of what is taking a chosen volume's space, walked on demand with a
  progress read and a cancel, the free space drawn beside the folders, and whatever could not be
  read shown as unknown rather than folded into either. Read-only: it names, it never deletes.
- **Memory and CPU maps** — treemaps of the programs holding RAM and using the CPU. The memory
  map is sized by working set and says shared pages count more than once; the CPU map is sized by
  a five-second average. The smaller apps fold into one box you can double-click to open.
- **Runaway** — dev processes stuck long enough to be worth naming (a core pinned for a quarter of
  an hour, a leak past 8 GB and still climbing, a server holding a port whose project is gone),
  each with the evidence and a way to its detail. It never ends anything itself.
- **Keep awake** — an off-by-default switch that asks Windows not to sleep while Meerkat runs, and
  lets go the moment it exits or the switch goes back.
- **Bluetooth** — connected devices with the batteries that can honestly be read: a percentage from
  the standard service for the ones that expose it, and the reason there is no number for the ones
  that do not (AirPods and their private protocol), never a made-up zero.
- **Worth a look** — apps that keep a core busy, grow in memory over half an hour, hammer the disk
  or the network, plus a toast for each (de-duplicated, with a cool-down).
- **Sound mixer** — per-application volume and mute with live level meters, remembered across
  restarts.
- **AI assistants (off until you turn it on)** — Settings → General prints the line that adds
  Meerkat to Claude Code and other MCP clients. They can ask what is using the machine, what
  holds a port, what an app did this week and what Windows logged. It is read-only: the one
  action, quitting an app, is a question the user answers in Meerkat's own window.
- **Game performance sessions.** Record one app or the foreground window while you play. Read a
  report afterwards with the findings and the fluctuation of every metric.

Nothing is faked: a reading Windows does not give an unprivileged process is shown as unavailable
instead of as zero. Processes are read from the kernel's own table, so Defender, Windows Update
and every service host are measured and named without elevation — service hosts for the services
they run, which is what Windows calls them in its own language. Per-app network *bytes* need an
ETW session only an administrator can open. The CPU package temperature is not reachable by any
user-mode Windows API at all — the one ACPI thermal zone this machine exposes reports a static
chassis reading that stays put under a full load, so it is shown as unavailable rather than
passed off as the processor; the GPU's own temperature, fan and live power come from the vendor
library without elevation.

Meerkat has no account, no telemetry and makes no network requests.

## Game performance sessions

Open the Performance tab. Choose the app you want to watch, or leave Meerkat following the
foreground window. Press Start before the game and Stop when you finish. The tab keeps a live
readout while a session runs and lists finished sessions underneath. Open one to see the verdict,
the findings and a chart for every metric.

CPU and GPU usage, memory use, VRAM use and GPU temperature come from Windows and the vendor
library. A reading that cannot be made stays unavailable. Two readings need a source outside
Meerkat.

FPS comes from a PresentMon CSV when PresentMon is writing one to `presentmon.csv` in Meerkat's
data directory. Start that capture before the session. Without it the report marks FPS as
unavailable and leaves out FPS findings. Meerkat never uses the monitor refresh rate as a
substitute for a measured frame rate.

CPU package temperature has no user-mode API on Windows, so the report leaves it unavailable
rather than passing a chassis sensor off as the processor. GPU temperature comes from NVML on
NVIDIA cards.

Sessions are stored in `perf-sessions.json` in the data directory. The newest 30 are kept.

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
- `MEERKAT_DATA_DIR=<dir> pnpm tauri dev` (a debug build only) keeps its data somewhere else, so
  a test instance can run beside the one in daily use.
- `MEERKAT_DEV_MIRROR=1 pnpm tauri dev` makes the debug backend mirror its state into
  `%APPDATA%\com.tzz.meerkat\dev`, so `pnpm dev` in a plain browser shows the same live figures.
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
pnpm package:portable     # zip with meerkat.exe and WebView2Loader.dll
```

## Where things live

User data is in `%APPDATA%\com.tzz.meerkat\`:

| File | What |
|---|---|
| `settings.json` | versioned and migrated on load (the previous version is kept once as `settings.v<N>.json.bak`); an unreadable file is kept as `settings.json.bad` |
| `history.json` | the machine as a whole: one-minute points for two hours, five-minute for a day, hourly for 30 days |
| `apps.db` | what each *app* did, per minute for a day and per ten minutes for 30, in SQLite |
| `daily.json` | bytes moved per local day, for the 7- and 30-day totals |
| `insights.json` | the "worth a look" list |
| `perf-sessions.json` | finished game sessions, their samples and their reports, newest 30 kept |
| `rules.json` | how an app should run (efficiency mode, priority, muted alerts) |
| `audio-volumes.json` | remembered per-app volumes |
| `logs/meerkat.log` | diagnostics (rotated at 512 KiB); the last lines are in Settings → About → Copy report |

A folder left by the build that was called Pulse (`com.tzz.pulse`) is adopted on the first launch
of this one, for any file this build does not have yet. It is never overwritten, and it is never
deleted.

Source layout:

- `src-tauri/src` — `metrics/` (sampler, `ntproc` (the kernel's process table), `grouping`
  (processes → apps), `services`, `projects`, PDH counters, GPU, battery, network), `history`
  (the machine), `apphistory` (per app, SQLite), `daily`, `events` (Windows' event log), `rules`
  + `control` (how an app runs), `alerts`, `settings`, `audio` (Core Audio), `tray*` / `menu`
  (notification-area icons and the panel), `quit` (what may be closed and how), `toast`, `mcp`,
  `i18n`, `diag`, `ipc`.
- `src` — `App.tsx` (window), `panel/` (tray panel), `components/`, `lib/` (store, IPC, pure
  helpers with tests), `i18n/`.
