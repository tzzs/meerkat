import type { AudioState, HistorySeries, Insight, RankingRow, Settings, Snapshot } from "./types";

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

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  if (inTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      return (await invoke<T>(command, args)) as T;
    } catch (error) {
      console.error(`${command} failed`, error);
      return null;
    }
  }
  switch (command) {
    case "snapshot":
      return fixture<T>("latest.json");
    case "insights":
      return fixture<T>("insights.json");
    case "audio_state":
      return fixture<T>("audio.json");
    case "history_series": {
      const metric = String(args?.metric ?? "cpu");
      const range = Number(args?.rangeSeconds ?? 3600);
      return (await fixture<T>(`series-${metric}-${range}.json`)) ?? null;
    }
    default:
      return null;
  }
}

export const ipc = {
  enabled: inTauri,
  snapshot: () => call<Snapshot>("snapshot"),
  settingsGet: () => call<Settings>("settings_get"),
  settingsSet: (settings: Settings) => call<Settings>("settings_set", { next: settings }),
  historySeries: async (metric: string, rangeSeconds: number) =>
    (await call<HistorySeries>("history_series", { metric, rangeSeconds })) ??
    emptySeries(metric, rangeSeconds),
  historyRanking: async (hours: number) =>
    (await call<RankingRow[]>("history_ranking", { hours })) ?? [],
  insights: async () => (await call<Insight[]>("insights")) ?? [],
  dismissInsight: (id: string) => void call("insight_dismiss", { id }),
  quitApp: (key: string, force: boolean) =>
    call<{ asked: number; forceUsed: boolean; error: string | null }>("quit_app", { key, force }),
  trayIds: async () => (await call<string[]>("tray_ids")) ?? [],
  setTrayBitmap: (id: string, png: number[]) => call("set_tray_bitmap", { id, png }),
  hidePanel: () => void call("hide_panel"),
  showWindow: (destination?: string) => void call("show_window", { destination: destination ?? null }),
  setPanelSize: (width: number, height: number) => void call("set_panel_size", { width, height }),
  setPanelPinned: (pinned: boolean) => void call("set_panel_pinned", { pinned }),
  copyReport: async () => (await call<string>("copy_report")) ?? "",
  version: async () => (await call<string>("app_version")) ?? "dev",
  audioState: () => call<AudioState>("audio_state"),
  audioSetVolume: (role: string, id: number, pid: number, volume: number, muted: boolean) =>
    call<string>("audio_set_volume", { role, id, pid, volume, muted }),
  audioSetEndpoint: (endpointId: string, volume: number, muted: boolean) =>
    call<string>("audio_set_endpoint", { endpointId, volume, muted }),
  /**
   * A real application icon as a data URL, or null when the executable has no readable icon.
   * Callers cache per path: extraction makes the shell load the module, so the first ask for
   * a given program is the expensive one.
   */
  appIcon: (exePath: string, size: number) => call<string | null>("app_icon", { exePath, size }),
  /**
   * Restarts the app with administrative rights. Returns the reason it did not happen: when
   * the user agrees, the process is already exiting and nothing is rendered in between.
   */
  relaunchElevated: async (): Promise<string | null> => {
    if (!inTauri) return null;
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      await invoke("relaunch_elevated");
      return null;
    } catch (error) {
      return typeof error === "string" ? error : String(error);
    }
  },
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

export async function listenMetrics(handler: (snapshot: Snapshot) => void): Promise<() => void> {
  if (inTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<Snapshot>("metrics", (event) => handler(event.payload));
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
  return listen<T>(name, (event) => handler(event.payload));
}

function emptySeries(metric: string, rangeSeconds: number): HistorySeries {
  return { metric, bucketSeconds: 60, rangeSeconds, points: [] };
}
