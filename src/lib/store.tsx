import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { makeT, resolveLang, type Translate } from "../i18n";
import type { Lang } from "../i18n";
import { setCores, type UnitPrefs } from "./format";
import { ipc, listenMetrics } from "./ipc";
import type { AppInfo, Settings, Snapshot } from "./types";

export const fallbackSettings: Settings = {
  version: 1,
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
    cpuPercent: 80,
    memoryGrowBytes: 1073741824,
    diskBytesPerSecond: 40000000,
    networkBytesPerSecond: 20000000,
    sustainedSeconds: 30,
  },
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
  save: (next: Settings) => void;
  units: UnitPrefs;
  lang: Lang;
  t: Translate;
  connected: boolean;
  /** Vitals asks before anything is closed; so does Pulse. */
  pendingQuit: AppInfo | null;
  requestQuit: (app: AppInfo) => void;
  clearQuit: () => void;
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

const StoreContext = createContext<StoreValue | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [settings, setSettings] = useState<Settings>(fallbackSettings);
  const [connected, setConnected] = useState(false);
  const [pendingQuit, setPendingQuit] = useState<AppInfo | null>(null);
  const [detailKey, setDetailKey] = useState<string | null>(null);
  const [detailSeed, setDetailSeed] = useState<AppSeed | null>(null);

  useEffect(() => {
    let stop: (() => void) | null = null;
    void (async () => {
      const loaded = await ipc.settingsGet();
      if (loaded) setSettings(loaded);
      const first = await ipc.snapshot();
      if (first) {
        setSnapshot(first);
        setConnected(true);
      }
      stop = await listenMetrics((next) => {
        setSnapshot(next);
        setConnected(true);
      });
    })();
    return () => stop?.();
  }, []);

  const save = useCallback((next: Settings) => {
    setSettings(next);
    void ipc.settingsSet(next);
  }, []);

  const lang = resolveLang(settings.language === "auto" ? "auto" : settings.language === "zhHans" ? "zh-Hans" : "en");
  const t = useMemo(() => makeT(lang), [lang]);
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

  const requestQuit = useCallback((app: AppInfo) => setPendingQuit(app), []);
  const clearQuit = useCallback(() => setPendingQuit(null), []);
  const openDetail = useCallback((key: string, seed?: AppSeed) => {
    setDetailKey(key);
    setDetailSeed(seed ?? null);
  }, []);
  const closeDetail = useCallback(() => setDetailKey(null), []);

  const value = useMemo(
    () => ({
      snapshot,
      settings,
      save,
      units,
      lang,
      t,
      connected,
      pendingQuit,
      requestQuit,
      clearQuit,
      detailKey,
      detailSeed,
      openDetail,
      closeDetail,
    }),
    [
      snapshot,
      settings,
      save,
      units,
      lang,
      t,
      connected,
      pendingQuit,
      requestQuit,
      clearQuit,
      detailKey,
      detailSeed,
      openDetail,
      closeDetail,
    ],
  );

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): StoreValue {
  const value = useContext(StoreContext);
  if (!value) throw new Error("useStore outside the provider");
  return value;
}

export function visibleSections(settings: Settings, tab: string): Set<string> {
  const config = settings.window.tabs.find((t) => t.id === tab);
  if (!config || config.sections.length === 0) return new Set(["*"]);
  return new Set(config.sections.filter((s) => s.visible).map((s) => s.id));
}

export function sectionShown(shown: Set<string>, id: string): boolean {
  return shown.has("*") || shown.has(id);
}
