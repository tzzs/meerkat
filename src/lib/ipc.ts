import type { LogLevel } from "./report";
import type {
  AppPoint,
  AppRule,
  AppUsage,
  BluetoothDevice,
  AudioState,
  DailyTotal,
  HistorySeries,
  Insight,
  PerfReport,
  PerfSession,
  PerfSessionSummary,
  PerfStatus,
  PerfTargetMode,
  ProcessRow,
  QuitResult,
  RankBy,
  RankingRow,
  RunawayVerdict,
  Settings,
  Snapshot,
  StorageMap,
  SystemEvent,
  WslMemory,
} from "./types";

const inTauri = "__TAURI_INTERNALS__" in window;

/** Outside Tauri, `pnpm dev` serves the debug build's mirrored state from /fixtures. */
async function fixture<T>(name: string): Promise<T | null> {
  try {
    const response = await fetch(`/fixtures/${name}`, { cache: "no-store" });
    if (!response.ok) return null;
    return (await response.json()) as T;
  } catch {
    return null;
  }
}

/**
 * What a command answered, or why it did not. `call` below folds a failure into null, which is
 * right for a reading that simply is not there yet and wrong for a write: a settings save or a
 * volume change that was refused has to be able to say so.
 */
export type IpcResult<T> = { ok: true; value: T | null } | { ok: false; error: string };

async function attempt<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  if (!inTauri) return { ok: true, value: await fixtureFor<T>(command, args) };
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return { ok: true, value: (await invoke<T>(command, args)) as T };
  } catch (error) {
    console.error(`${command} failed`, error);
    return { ok: false, error: typeof error === "string" ? error : String(error) };
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  const result = await attempt<T>(command, args);
  return result.ok ? result.value : null;
}

function fixtureFor<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  switch (command) {
    case "snapshot":
      return fixture<T>("latest.json");
    case "insights":
      return fixture<T>("insights.json");
    case "audio_state":
      return fixture<T>("audio.json");
    case "history_ranking":
      return fixture<T>("ranking.json");
    case "history_series": {
      const metric = String(args?.metric ?? "cpu");
      const range = Number(args?.rangeSeconds ?? 3600);
      return fixture<T>(`series-${metric}-${range}.json`);
    }
    default:
      return Promise.resolve(null);
  }
}

export const ipc = {
  enabled: inTauri,
  snapshot: () => call<Snapshot>("snapshot"),
  settingsGet: () => call<Settings>("settings_get"),
  /**
   * Saves and answers with what was stored, which the backend may have corrected (a threshold
   * clamped into range), or with the reason it was not stored at all.
   */
  settingsSave: (settings: Settings) => attempt<Settings>("settings_set", { next: settings }),
  historySeries: async (metric: string, rangeSeconds: number) =>
    (await call<HistorySeries>("history_series", { metric, rangeSeconds })) ??
    emptySeries(metric, rangeSeconds),
  historyRanking: async (hours: number) =>
    (await call<RankingRow[]>("history_ranking", { hours })) ?? [],
  /** The heaviest apps over the last `rangeSeconds`, by one measure. */
  appRanking: async (rangeSeconds: number, by: RankBy) =>
    (await call<AppUsage[]>("app_ranking", { rangeSeconds, by })) ?? [],
  /** The heaviest apps since local midnight. */
  appRankingToday: async (by: RankBy) => (await call<AppUsage[]>("app_ranking_today", { by })) ?? [],
  /** What was running at one moment, busiest first. */
  appsAt: async (atMs: number) => (await call<AppUsage[]>("apps_at", { atMs })) ?? [],
  appSeries: async (key: string, rangeSeconds: number) =>
    (await call<AppPoint[]>("app_series", { key, rangeSeconds })) ?? [],
  systemEvents: async (rangeSeconds: number) => (await call<SystemEvent[]>("system_events", { rangeSeconds })) ?? [],
  rules: async () => (await call<AppRule[]>("rules_list")) ?? [],
  /** Stores an app's rule and applies it at once; null when it worked, else the reason. */
  setRule: async (rule: AppRule): Promise<string | null> => {
    const result = await attempt<null>("rule_set", { rule });
    return result.ok ? null : result.error;
  },
  /** Pauses or resumes an app; null when it worked, else the reason. */
  pauseApp: async (key: string, pause: boolean): Promise<string | null> => {
    const result = await attempt<number>("app_pause", { key, pause });
    return result.ok ? null : result.error;
  },
  /** The processes behind one app row, one each, heaviest first. */
  appProcesses: async (key: string) => (await call<ProcessRow[]>("app_processes", { key })) ?? [],
  /** Brings one of an app's windows to the front; null when it worked, else the reason. */
  focusWindow: async (hwnd: number): Promise<string | null> => {
    const result = await attempt<null>("focus_window", { hwnd });
    return result.ok ? null : result.error;
  },
  revealInExplorer: (path: string) => void call("reveal_in_explorer", { path }),
  /** Opens the project folder of a runtime grouped by project. */
  openProject: (key: string) => void call("open_project", { key }),
  /** The command that adds this Meerkat to Claude Code as an MCP server. */
  mcpCommand: async () => (await call<string>("mcp_command")) ?? "",
  /** The WSL virtual machine's memory as Linux sees it; null without WSL running. */
  wslMemory: () => call<WslMemory>("wsl_memory"),
  /** Connected Bluetooth devices and their batteries; a slow radio read, so asked on demand. */
  bluetoothDevices: async () => (await call<BluetoothDevice[]>("bluetooth_devices")) ?? [],
  /**
   * Maps what is taking the space on one volume. The walk can take minutes on a full drive, so
   * it runs only when asked and answers with the result (or the reason it could not).
   */
  storageMapScan: (volumeId: string) => attempt<StorageMap>("storage_map_scan", { volumeId }),
  /** Abandons the storage-map scan in flight. */
  storageMapCancel: () => void call("storage_map_cancel"),
  /** Bytes per local day, oldest first, zero for days Meerkat was not running. */
  dailyTotals: async (days: number) => (await call<DailyTotal[]>("daily_totals", { days })) ?? [],
  insights: async () => (await call<Insight[]>("insights")) ?? [],
  dismissInsight: (id: string) => void call("insight_dismiss", { id }),
  /** The watchdog's current findings, fetched when a hidden window is shown again. */
  runaway: async () => (await call<RunawayVerdict[]>("runaway")) ?? [],
  dismissRunaway: (id: string) => void call("runaway_dismiss", { id }),
  quitApp: (key: string, force: boolean) => call<QuitResult>("quit_app", { key, force }),
  /** Starts a game session. Null means it started; a string is the refusal to show. */
  perfStart: async (mode: PerfTargetMode, appKey: string | null): Promise<string | null> => {
    const result = await attempt<null>("perf_start", { mode, appKey });
    return result.ok ? null : result.error;
  },
  /** The report answers through IpcResult so a refused stop is not shown as a clean report. */
  perfStop: () => attempt<PerfReport>("perf_stop"),
  perfStatus: async () => (await call<PerfStatus>("perf_status")) ?? emptyPerfStatus(),
  perfSessions: async () => (await call<PerfSessionSummary[]>("perf_sessions")) ?? [],
  perfReport: (id: string) => call<PerfSession>("perf_report", { id }),
  perfDelete: (id: string) => call("perf_delete", { id }),
  /**
   * Writes a line to Meerkat's own log. Never throws and never reports its own failure through
   * the error path it serves, so a broken bridge cannot turn one error into a loop.
   */
  logFrontend: (level: LogLevel, message: string) => {
    if (!inTauri) return;
    void import("@tauri-apps/api/core")
      .then(({ invoke }) => invoke("log_frontend", { level, message }))
      .catch(() => undefined);
  },
  /**
   * Ends Meerkat: the tray readouts and the sampling with it. Closing the window only hides it,
   * so this is the one way out from the window itself.
   */
  quitMeerkat: () => void call("quit_meerkat"),
  hidePanel: () => void call("hide_panel"),
  showWindow: (destination?: string) => void call("show_window", { destination: destination ?? null }),
  setPanelSize: (width: number, height: number) => void call("set_panel_size", { width, height }),
  setPanelPinned: (pinned: boolean) => void call("set_panel_pinned", { pinned }),
  copyReport: async () => (await call<string>("copy_report")) ?? "",
  version: async () => (await call<string>("app_version")) ?? "dev",
  audioState: () => call<AudioState>("audio_state"),
  /**
   * The mixer's writes answer with the reason Windows refused them (a device that went away, a
   * session that just ended), or null when it took them. They went through `call`, which folds a
   * failure into the same null a success has, so a refused change was indistinguishable from an
   * accepted one and the slider simply sat where it was left.
   */
  audioSetVolume: async (
    role: string,
    id: number,
    pid: number,
    volume: number,
    muted: boolean,
  ): Promise<string | null> => {
    const result = await attempt<null>("audio_set_volume", { role, id, pid, volume, muted });
    return result.ok ? null : result.error;
  },
  audioSetEndpoint: async (endpointId: string, volume: number, muted: boolean): Promise<string | null> => {
    const result = await attempt<null>("audio_set_endpoint", { endpointId, volume, muted });
    return result.ok ? null : result.error;
  },
  /**
   * A real application icon as a data URL, or null when the executable has no readable icon.
   * Callers cache per path: extraction makes the shell load the module, so the first ask for
   * a given program is the expensive one.
   */
  appIcon: (exePath: string, size: number) => call<string | null>("app_icon", { exePath, size }),
  /**
   * Window management for the frameless main window. No-ops outside Tauri so a plain
   * browser mirror of the UI still renders.
   */
  minimize: async () => {
    if (inTauri) (await import("@tauri-apps/api/window")).getCurrentWindow().minimize();
  },
  toggleMaximize: async () => {
    if (inTauri) (await import("@tauri-apps/api/window")).getCurrentWindow().toggleMaximize();
  },
  /** Close, which the app intercepts to hide to the tray rather than quit. */
  closeWindow: async () => {
    if (inTauri) (await import("@tauri-apps/api/window")).getCurrentWindow().close();
  },
  isMaximized: async () => {
    if (!inTauri) return false;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return getCurrentWindow().isMaximized();
  },
};

/**
 * The label of the window this script runs in ("main" or "panel").
 *
 * The backend sends a tick only to the window somebody can see (`emit_to`). A listener that
 * registers for the default "any" target hears every `emit_to`, whoever it was addressed to, so
 * the window nobody is looking at would still parse and render each tick meant for the other
 * one. A listener scoped to its own label ignores those, and still hears global broadcasts such
 * as `settings-changed`.
 */
async function ownLabel(): Promise<string> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().label;
}

export async function listenMetrics(handler: (snapshot: Snapshot) => void): Promise<() => void> {
  if (inTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<Snapshot>("metrics", (event) => handler(event.payload), { target: await ownLabel() });
  }
  const timer = window.setInterval(async () => {
    const next = await fixture<Snapshot>("latest.json");
    if (next) handler(next);
  }, 1000);
  return () => window.clearInterval(timer);
}

export async function listenNavigate(handler: (destination: string) => void): Promise<() => void> {
  return listenEvent<string>("navigate", handler);
}

/**
 * Any backend event by name. Outside Tauri nothing is ever emitted, so the subscription is a
 * no-op rather than an error: the dev browser simply never hears these.
 */
export async function listenEvent<T = unknown>(name: string, handler: (payload: T) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(name, (event) => handler(event.payload), { target: await ownLabel() });
}

function emptyPerfStatus(): PerfStatus {
  return {
    active: false,
    sessionId: null,
    targetName: "",
    mode: "foreground",
    currentAppKey: null,
    sampleCount: 0,
    startedAtMs: null,
    latest: null,
  };
}

function emptySeries(metric: string, rangeSeconds: number): HistorySeries {
  return { metric, bucketSeconds: 60, rangeSeconds, points: [] };
}

