import { useEffect, useMemo, useState } from "react";
import { formatBytes, formatDuration } from "./lib/format";
import { listenNavigate } from "./lib/ipc";
import { TAB_LABEL } from "./i18n";
import { useStore } from "./lib/store";
import { powerTabMode, type TabId } from "./lib/types";
import { BatteryTab, CpuTab, DiskTab, GpuTab, MemoryTab, NetworkTab, OverviewTab } from "./components/tabs";
import { QuitDialog } from "./components/ui";
import { SettingsPages } from "./components/settings";
import { SoundTab } from "./components/sound";

export default function App() {
  const { snapshot, settings, t, units, connected, pendingQuit } = useStore();
  const [tab, setTab] = useState(
    () => new URLSearchParams(window.location.search).get("tab") || settings.openWindowOn || "overview",
  );
  const [search, setSearch] = useState("");
  const [page, setPage] = useState<null | "general" | "layout" | "about">(null);

  // A desktop has no battery; the power tab keeps its temperature content and is called
  // Temperature instead, and only vanishes when there is neither.
  const powerMode = powerTabMode(snapshot);

  useEffect(() => {
    document.documentElement.dataset.pulse = "window";
  }, []);

  useEffect(() => {
    let stop: (() => void) | null = null;
    void listenNavigate((destination) => {
      if (destination === "settings") setPage("general");
      else setTab(destination);
    }).then((un) => (stop = un));
    return () => stop?.();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setPage(null);
        setTab("memory");
        window.setTimeout(() => document.querySelector<HTMLInputElement>(".titlebar .find")?.focus(), 30);
      }
      if (event.key === "Escape") {
        // The quit dialog owns Escape while it is up; it should not also drop out of Settings.
        if (pendingQuit) return;
        if (page) setPage(null);
      }
      const digits = "1234567";
      if ((event.ctrlKey || event.metaKey) && digits.includes(event.key)) {
        const visible = settings.window.tabs.filter((x) => x.visible);
        const target = visible[Number(event.key) - 1];
        if (target) {
          event.preventDefault();
          setPage(null);
          setTab(target.id);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settings.window.tabs, page, pendingQuit]);

  useEffect(() => {
    // Being left on a tab that the hardware just stopped justifying looks like a broken window.
    if (powerMode === "hidden" && tab === "battery") setTab("overview");
  }, [powerMode, tab]);

  const tabs = settings.window.tabs.filter((x) => x.visible && !(x.id === "battery" && powerMode === "hidden"));
  const uptime = snapshot ? formatDuration(snapshot.machine.uptimeSeconds, units.locale, { short: true }) : "—";

  const summary = useMemo(() => {
    if (!snapshot) return "";
    const m = snapshot.machine;
    return [
      m.chipName,
      `${m.physicalCores ?? "?"}/${m.logicalCores}`,
      formatBytes(m.totalMemoryBytes, units),
      `up ${uptime}`,
    ]
      .filter(Boolean)
      .join("  ·  ");
  }, [snapshot, units, uptime]);

  return (
    <div className="shell">
      <header className="titlebar">
        <span className="brand">
          <span className="brand-dot" />
          Pulse
        </span>
        <span className="machine">{summary}</span>
        <span className="spacer" />
        <input
          className="find"
          placeholder={`${t("action.search")} (Ctrl+F)`}
          value={search}
          onChange={(e) => {
            setSearch(e.target.value);
            if (e.target.value) {
              setPage(null);
              setTab("memory");
            }
          }}
        />
        <button className="tool" onClick={() => setPage(page === "layout" ? null : "layout")} title={t("action.arrange")}>
          ▦
        </button>
        <button className="tool" onClick={() => setPage(page === "general" ? null : "general")} title={t("action.settings")}>
          ⚙
        </button>
      </header>

      <nav className="tabbar" role="tablist">
        {tabs.map((x) => (
          <button
            key={x.id}
            role="tab"
            className="tab"
            aria-selected={!page && tab === x.id}
            onClick={() => {
              setPage(null);
              setTab(x.id);
            }}
          >
            {t(x.id === "battery" && powerMode === "thermal" ? "metric.temperature" : TAB_LABEL[x.id] ?? "tab.overview")}
          </button>
        ))}
        <span className="spacer" />
        <span className={`status ${connected ? "live" : "off"}`}>
          {connected ? t("app.name") : t("state.noData")}
        </span>
      </nav>

      <main className="content">
        {page ? (
          <SettingsPages page={page} setPage={setPage} />
        ) : (
          <>
            {tab === "overview" && <OverviewTab onOpenTab={(next) => setTab(next as TabId)} />}
            {tab === "cpu" && <CpuTab />}
            {tab === "memory" && <MemoryTab search={search} />}
            {tab === "disk" && <DiskTab />}
            {tab === "network" && <NetworkTab />}
            {tab === "gpu" && <GpuTab />}
            {tab === "battery" && <BatteryTab />}
            {tab === "sound" && <SoundTab />}
          </>
        )}
      </main>

      <footer className="statusbar">
        <span className="faint">
          {snapshot
            ? `${snapshot.apps.length} ${t("label.apps").toLowerCase()} · ${snapshot.processCount} ${t("label.processes").toLowerCase()}`
            : ""}
        </span>
        <span className="spacer" />
        <span className="faint num">
          {snapshot ? `${(snapshot.samplerCostMicroseconds / 1000).toFixed(1)} ms/tick` : ""}
        </span>
      </footer>
      <QuitDialog />
    </div>
  );
}
