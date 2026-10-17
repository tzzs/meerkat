import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { makeT, resolveLang, type Translate } from "../i18n";
import type { Lang } from "../i18n";
import { setCores, type UnitPrefs } from "./format";
import { ipc, listenEvent, listenMetrics } from "./ipc";
import type { ToastTone } from "./quit";
import { subscribe } from "./subscribe";
import type { AppInfo, Settings, Snapshot } from "./types";

/** What the window shows before the backend has answered. Mirrors `Settings::default()` in Rust. */
export const fallbackSettings: Settings = {
  version: 2,
  language: "auto",
  temperatureUnit: "celsius",
  networkUnit: "bytes",
  storageUnit: "decimal",
  cpuScale: "perMachine",
  theme: "auto",
  showSystemAppsInLists: true,
  openWindowOn: "overview",
  startAtLogin: true,
  showInTaskbar: false,
  tray: [
    { id: "cpu", metric: "cpu", style: "both", showIcon: false, showCaption: false, invert: false, enabled: true },
    { id: "memory", metric: "memory", style: "value", showIcon: false, showCaption: false, invert: false, enabled: true },
    { id: "network", metric: "network", style: "graph", showIcon: false, showCaption: false, invert: false, enabled: false },
  ],
  panel: {
    layout: "tiles",
    tiles: [
      { id: "cpu", visible: true },
      { id: "memory", visible: true },
      { id: "disk", visible: true },
      { id: "network", visible: true },
      { id: "gpu", visible: true },
      { id: "battery", visible: true },
    ],
  },
  window: {
    tabs: [
      { id: "overview", visible: true, sections: [] },
      { id: "history", visible: true, sections: [] },
      { id: "cpu", visible: true, sections: [] },
      { id: "memory", visible: true, sections: [] },
      { id: "disk", visible: true, sections: [] },
      { id: "network", visible: true, sections: [] },
      { id: "gpu", visible: true, sections: [] },
      { id: "battery", visible: true, sections: [] },
      { id: "sound", visible: true, sections: [] },
    ],
  },
  alerts: {
    enabled: true,
    // Percent of the whole machine, not of one core.
    cpuPercent: 25,
    memoryGrowBytes: 1073741824,
    diskBytesPerSecond: 40000000,
    networkBytesPerSecond: 20000000,
    sustainedSeconds: 30,
  },
  mcpEnabled: false,
};

/** Enough to name an app that is not running: its key, label, file and kind. */
export interface AppSeed {
  key: string;
  name: string;
  exePath: string;
  kind: AppInfo["kind"];
}

interface StoreValue {
  snapshot: Snapshot | null;
  settings: Settings;
  /** True once the backend has answered with real settings, not the fallback above. */
  settingsLoaded: boolean;
  save: (next: Settings) => void;
  /** Why the last save was refused (it has been rolled back), until dismissed or saved over. */
  settingsError: string | null;
  clearSettingsError: () => void;
  units: UnitPrefs;
  lang: Lang;
  t: Translate;
  connected: boolean;
  /** Vitals asks before anything is closed; so does Meerkat. */
  pendingQuit: QuitRequest | null;
  /** `preferForce` opens the dialog with force quit as the highlighted action. */
  requestQuit: (app: AppInfo, preferForce?: boolean) => void;
  clearQuit: () => void;
  toasts: Toast[];
  pushToast: (toast: Omit<Toast, "id">) => void;
  dismissToast: (id: number) => void;
  /** The app whose detail sheet is open, by key, so it follows the app through every sample. */
  detailKey: string | null;
  /**
   * What to call it when it is not running: the history and the event list can open apps that
   * have since exited, and the live snapshot knows nothing about those.
   */
  detailSeed: AppSeed | null;
  openDetail: (key: string, seed?: AppSeed) => void;
  closeDetail: () => void;
}

export interface QuitRequest {
  app: AppInfo;
  preferForce: boolean;
}

/** A short notice that does not block anything: the outcome of an action, mostly. */
export interface Toast {
  id: number;
  text: string;
  tone: ToastTone;
  action?: { label: string; run: () => void };
}

/** Long enough to read a sentence; a notice with a button stays a little longer. */
const TOAST_MS = 6000;
const TOAST_WITH_ACTION_MS = 10_000;

const StoreContext = createContext<StoreValue | null>(null);

/** Keeps the newer of two snapshots: an answer to `snapshot` can land after a pushed one. */
function newer(previous: Snapshot | null, next: Snapshot): Snapshot {
  return previous && previous.takenAtMs > next.takenAtMs ? previous : next;
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [settings, setSettings] = useState<Settings>(fallbackSettings);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  const [pendingQuit, setPendingQuit] = useState<QuitRequest | null>(null);
  const [detailKey, setDetailKey] = useState<string | null>(null);
  const [detailSeed, setDetailSeed] = useState<AppSeed | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastSeq = useRef(0);
  // Saves the backend has not answered yet. A settings value fetched meanwhile (because some
  // event asked for one) can predate them, and applying it would undo what was just chosen.
  const savesInFlight = useRef(0);
  const saveSeq = useRef(0);
  // The last value the backend confirmed, which is what a refused save rolls back to.
  const confirmed = useRef<Settings>(fallbackSettings);

  const loadSettings = useCallback(async () => {
    const loaded = await ipc.settingsGet();
    if (!loaded || savesInFlight.current > 0) return;
    confirmed.current = loaded;
    setSettings(loaded);
    setSettingsLoaded(true);
  }, []);

  useEffect(() => {
    let alive = true;
    const loadSnapshot = async () => {
      const next = await ipc.snapshot();
      if (!alive || !next) return;
      setSnapshot((previous) => newer(previous, next));
      setConnected(true);
    };
    // The backend only pushes snapshots to a window that is on screen, so one that has just
    // been shown is up to a minute stale and catches up at once rather than at the next tick.
    // Its settings may be older still: the panel lives for the whole session and used to keep
    // the language, theme and units it was started with until Meerkat restarted.
    const catchUp = () => {
      void loadSnapshot();
      void loadSettings();
    };
    catchUp();
    const stops = [
      subscribe(() =>
        listenMetrics((next) => {
          if (!alive) return;
          setSnapshot((previous) => newer(previous, next));
          setConnected(true);
        }),
      ),
      subscribe(() => listenEvent("settings-changed", () => void loadSettings())),
      subscribe(() => listenEvent("panel-shown", catchUp)),
      subscribe(() => listenEvent("window-shown", catchUp)),
    ];
    return () => {
      alive = false;
      for (const stop of stops) stop();
    };
  }, [loadSettings]);

  const save = useCallback((next: Settings) => {
    const seq = ++saveSeq.current;
    savesInFlight.current += 1;
    setSettings(next);
    setSettingsError(null);
    void ipc.settingsSave(next).then(async (result) => {
      savesInFlight.current -= 1;
      // A later save owns the state now; its own answer will settle it.
      if (seq !== saveSeq.current) return;
      if (result.ok) {
        // What Rust stored after sanitising, which is not always what was sent. Outside Tauri
        // there is no backend to answer, and the choice simply stands.
        if (result.value) {
          confirmed.current = result.value;
          setSettings(result.value);
        }
        return;
      }
      // Refused: say so, and put back what is really in effect instead of leaving a control
      // showing a value Meerkat is not using.
      setSettingsError(result.error);
      const current = await ipc.settingsGet();
      if (seq !== saveSeq.current || savesInFlight.current > 0) return;
      if (current) confirmed.current = current;
      setSettings(confirmed.current);
    });
  }, []);

  const clearSettingsError = useCallback(() => setSettingsError(null), []);

  const lang = resolveLang(settings.language === "auto" ? "auto" : settings.language === "zhHans" ? "zh-Hans" : "en");
  const t = useMemo(() => makeT(lang), [lang]);

  // The pages ship as lang="en" so the first paint has a language; this follows the setting,
  // which is what screen readers and CJK font selection read.
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);
  const units = useMemo<UnitPrefs>(
    () => ({
      storage: settings.storageUnit,
      network: settings.networkUnit,
      temperature: settings.temperatureUnit,
      cpuScale: settings.cpuScale,
      locale: lang === "zh-Hans" ? "zh-CN" : "en-US",
    }),
    [settings.storageUnit, settings.networkUnit, settings.temperatureUnit, settings.cpuScale, lang],
  );

  useEffect(() => {
    if (snapshot) setCores(snapshot.machine.logicalCores);
  }, [snapshot]);

  useEffect(() => {
    const root = document.documentElement;
    if (settings.theme === "auto") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", settings.theme);
    // shadcn's components carry `dark:` utilities. Tailwind v4 resolves that variant against the
    // operating system unless a class decides it, and here the theme is decided by a setting —
    // so the class is written from the same reading Windows would otherwise disagree with.
    const light = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      const dark = settings.theme === "dark" || (settings.theme === "auto" && !light.matches);
      root.classList.toggle("dark", dark);
    };
    apply();
    if (settings.theme !== "auto") return;
    light.addEventListener("change", apply);
    return () => light.removeEventListener("change", apply);
  }, [settings.theme]);

  const requestQuit = useCallback(
    (app: AppInfo, preferForce = false) => setPendingQuit({ app, preferForce }),
    [],
  );
  const clearQuit = useCallback(() => setPendingQuit(null), []);
  const openDetail = useCallback((key: string, seed?: AppSeed) => {
    setDetailKey(key);
    setDetailSeed(seed ?? null);
  }, []);
  const closeDetail = useCallback(() => setDetailKey(null), []);

  const dismissToast = useCallback((id: number) => setToasts((list) => list.filter((toast) => toast.id !== id)), []);
  const pushToast = useCallback(
    (toast: Omit<Toast, "id">) => {
      const id = ++toastSeq.current;
      // Three at most: a burst of outcomes should not stack up the side of the window.
      setToasts((list) => [...list, { ...toast, id }].slice(-3));
      window.setTimeout(() => dismissToast(id), toast.action ? TOAST_WITH_ACTION_MS : TOAST_MS);
    },
    [dismissToast],
  );

  const value = useMemo(
    () => ({
      snapshot,
      settings,
      settingsLoaded,
      save,
      settingsError,
      clearSettingsError,
      units,
      lang,
      t,
      connected,
      pendingQuit,
      requestQuit,
      clearQuit,
      toasts,
      pushToast,
      dismissToast,
      detailKey,
      detailSeed,
      openDetail,
      closeDetail,
    }),
    [
      toasts,
      pushToast,
      dismissToast,
      detailKey,
      detailSeed,
      openDetail,
      closeDetail,
      snapshot,
      settings,
      settingsLoaded,
      save,
      settingsError,
      clearSettingsError,
      units,
      lang,
      t,
      connected,
      pendingQuit,
      requestQuit,
      clearQuit,
    ],
  );

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): StoreValue {
  const value = useContext(StoreContext);
  if (!value) throw new Error("useStore outside the provider");
  return value;
}
