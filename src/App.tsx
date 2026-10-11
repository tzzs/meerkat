import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { formatBytes, formatDuration } from "./lib/format";
import { ipc, listenNavigate } from "./lib/ipc";
import { isTabId, openingTab, readLastTab, shortcutHint, tabForShortcut, writeLastTab } from "./lib/navigation";
import { subscribe } from "./lib/subscribe";
import { TAB_LABEL } from "./i18n";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./components/ui/tabs";
import { Button } from "./components/ui/button";
import { Hint } from "./components/ui/tooltip";
import { useStore } from "./lib/store";
import { powerTabMode, type TabId } from "./lib/types";
import { BatteryTab, CpuTab, DiskTab, GpuTab, MemoryTab, NetworkTab, OverviewTab } from "./components/tabs";
import { QuitDialog, Toaster } from "./components/ui";
import { SettingsPages } from "./components/settings";
import { SoundTab } from "./components/sound";

/** `?tab=` lets a probe open the window on a given tab; it outranks the setting. */
function tabFromUrl(): string | null {
  return new URLSearchParams(window.location.search).get("tab");
}

export default function App() {
  const { snapshot, settings, settingsLoaded, t, units, connected, pendingQuit } = useStore();
  const [tab, setTab] = useState(() => tabFromUrl() || "overview");
  // Read during the first render, before the effect below can record the tab the window
  // merely started on as if it had been the user's choice.
  const [launchLastTab] = useState(() => readLastTab());
  const openingApplied = useRef(false);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState<null | "general" | "layout" | "about">(null);
  const [maximized, setMaximized] = useState(false);

  // The window has no frame, so its own buttons have to show the real state.
  useEffect(() => {
    if (!ipc.enabled) return;
    let alive = true;
    const sync = () => void ipc.isMaximized().then((value) => alive && setMaximized(value));
    sync();
    window.addEventListener("resize", sync);
    return () => {
      alive = false;
      window.removeEventListener("resize", sync);
    };
  }, []);

  // A desktop has no battery; the power tab keeps its temperature content and is called
  // Temperature instead, and only vanishes when there is neither.
  const powerMode = powerTabMode(snapshot);

  // "Open window on" used to be read in the initial state, i.e. before the settings had loaded,
  // so it was always the fallback's Overview and the setting did nothing. It is applied once,
  // when the real settings first arrive.
  useEffect(() => {
    if (openingApplied.current || !settingsLoaded) return;
    openingApplied.current = true;
    if (tabFromUrl()) return;
    setTab(openingTab(settings.openWindowOn, launchLastTab, settings.window.tabs));
  }, [settingsLoaded, settings.openWindowOn, settings.window.tabs, launchLastTab]);

  useEffect(() => {
    if (openingApplied.current) writeLastTab(tab);
  }, [tab]);

  useEffect(
    () =>
      subscribe(() =>
        listenNavigate((destination) => {
          if (destination === "settings") setPage("general");
          else if (isTabId(destination)) {
            setPage(null);
            setTab(destination);
          }
        }),
      ),
    [],
  );

  // The tabs the bar shows, in its order: what Ctrl+digit counts along.
  const tabs = settings.window.tabs.filter((x) => x.visible && !(x.id === "battery" && powerMode === "hidden"));
  const shownIds = tabs.map((x) => x.id);
  const shownKey = shownIds.join(" ");

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setPage(null);
        setTab("memory");
        window.setTimeout(() => document.querySelector<HTMLInputElement>(".titlebar .find")?.focus(), 30);
      }
      if (event.key === "Escape") {
        // A dialog, a dropdown or a number box that took Escape for itself has marked it
        // handled, and it should not also drop out of Settings. The quit dialog is one of
        // those, but its request is cleared before the key reaches here, so it is named too.
        if (event.defaultPrevented || pendingQuit) return;
        if (page) setPage(null);
      }
      if (event.ctrlKey || event.metaKey) {
        const target = tabForShortcut(event.key, shownIds);
        if (target) {
          event.preventDefault();
          setPage(null);
          setTab(target);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shownKey, page, pendingQuit]);

  useEffect(() => {
    // Being left on a tab that the hardware just stopped justifying looks like a broken window.
    if (powerMode === "hidden" && tab === "battery") setTab("overview");
  }, [powerMode, tab]);

  const uptime = snapshot ? formatDuration(snapshot.machine.uptimeSeconds, units.locale, { short: true }) : "—";

  const summary = useMemo(() => {
    if (!snapshot) return "";
    const m = snapshot.machine;
    return [
      m.chipName,
      `${m.physicalCores ?? "?"}/${m.logicalCores}`,
      formatBytes(m.totalMemoryBytes, units),
      t("machine.up", { duration: uptime }),
    ]
      .filter(Boolean)
      .join("  ·  ");
  }, [snapshot, units, uptime, t]);

  return (
    <Tabs
      className="shell"
      // Nothing is selected while a Settings page is up; that is what the old
      // aria-selected={!page && ...} meant, and a tab that looks active while it is not
      // on screen is a lie the keyboard will repeat back to you.
      value={page ? "" : tab}
      onValueChange={(next: string) => {
        setPage(null);
        setTab(next as TabId);
      }}
    >
      <header
        className="titlebar"
        data-tauri-drag-region
        onDoubleClick={(event) => {
          // Only the draggable strip maximises; a double click in the search box is a
          // person selecting a word.
          if ((event.target as HTMLElement).hasAttribute("data-tauri-drag-region")) {
            void ipc.toggleMaximize();
          }
        }}
      >
        <span className="brand">
          <span className="brand-dot" />
          Pulse
        </span>
        <span className="machine" data-tauri-drag-region>
          {summary}
        </span>
        <span className="spacer" data-tauri-drag-region />
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
        <Hint label={t("action.arrange")}>
          <Button
            variant="tool"
            aria-label={t("action.arrange")}
            onClick={() => setPage(page === "layout" ? null : "layout")}
          >
            ▦
          </Button>
        </Hint>
        <Hint label={t("action.settings")}>
          <Button
            variant="tool"
            aria-label={t("action.settings")}
            onClick={() => setPage(page === "general" ? null : "general")}
          >
            ⚙
          </Button>
        </Hint>
        {ipc.enabled && (
          <span className="winctls">
            <Hint label={t("action.minimize")}>
              <button className="winctl" onClick={() => void ipc.minimize()} aria-label={t("action.minimize")}>
                ─
              </button>
            </Hint>
            <Hint label={maximized ? t("action.restore") : t("action.maximize")}>
              <button
                className="winctl"
                onClick={() => void ipc.toggleMaximize()}
                aria-label={maximized ? t("action.restore") : t("action.maximize")}
              >
                ▢
              </button>
            </Hint>
            {/* Closing hides the window; Pulse keeps watching from the tray. The button says
                so rather than implying it ends the program. */}
            <Hint label={t("action.hideToTray")}>
              <button
                className="winctl close"
                onClick={() => void ipc.closeWindow()}
                aria-label={t("action.hideToTray")}
              >
                ✕
              </button>
            </Hint>
          </span>
        )}
      </header>

      <div className="tabbar">
        <TabsList>
          {tabs.map((x, index) => {
            const trigger = (
              <TabsTrigger value={x.id}>
                {t(x.id === "battery" && powerMode === "thermal" ? "metric.temperature" : TAB_LABEL[x.id] ?? "tab.overview")}
              </TabsTrigger>
            );
            // Ctrl+1 to Ctrl+8 pick the tab at that place in the bar, so the bar says which.
            const hint = shortcutHint(index);
            return hint ? (
              <Hint key={x.id} label={hint}>
                {trigger}
              </Hint>
            ) : (
              <Fragment key={x.id}>{trigger}</Fragment>
            );
          })}
        </TabsList>
        <span className={`status ${connected ? "live" : "off"}`}>
          {connected ? t("status.live") : t("status.waiting")}
        </span>
      </div>

      <main className="content">
        {page ? (
          <SettingsPages page={page} setPage={setPage} />
        ) : (
          <>
            <TabsContent value="overview">
              <OverviewTab onOpenTab={(next) => setTab(next as TabId)} />
            </TabsContent>
            <TabsContent value="cpu">
              <CpuTab />
            </TabsContent>
            <TabsContent value="memory">
              <MemoryTab search={search} />
            </TabsContent>
            <TabsContent value="disk">
              <DiskTab />
            </TabsContent>
            <TabsContent value="network">
              <NetworkTab />
            </TabsContent>
            <TabsContent value="gpu">
              <GpuTab />
            </TabsContent>
            <TabsContent value="battery">
              <BatteryTab />
            </TabsContent>
            <TabsContent value="sound">
              <SoundTab />
            </TabsContent>
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
          {snapshot ? t("status.samplerCost", { ms: (snapshot.samplerCostMicroseconds / 1000).toFixed(1) }) : ""}
        </span>
      </footer>
      <QuitDialog />
      <Toaster />
    </Tabs>
  );
}
