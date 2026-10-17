import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppMenu, isMenuKey, menuTarget, type AppMenuTarget } from "../components/AppMenu";
import { appLabel, canQuit, forLists, holdOrder, orderApps } from "../lib/apps";
import { usePins } from "../lib/pins";
import { cleanChipName, cpuDisplay, formatBytes, formatPercent, formatRateShort, formatTemp } from "../lib/format";
import { ipc, listenEvent } from "../lib/ipc";
import { AppIcon, QuitDialog, Toaster, useHeldCeiling } from "../components/ui";
import { Hint, TooltipReset } from "../components/ui/tooltip";
import { useStore } from "../lib/store";
import { panelNav, panelSeries, panelTabForKey, panelTiles, panelWindowSize, type PanelTab } from "../lib/panel";
import { SPARK_CEILING, sparkPoints, windowPeak } from "../lib/spark";
import { subscribe } from "../lib/subscribe";
import { headline, type MetricKind, type Snapshot } from "../lib/types";

const HUES: Record<MetricKind, string> = {
  cpu: "#5ecb8f",
  memory: "#64a9ff",
  disk: "#f0b35f",
  network: "#4fd2da",
  gpu: "#ea7ac6",
  battery: "#a3d75f",
  temperature: "#ff8b5e",
};

export function PanelApp() {
  const { snapshot, settings, t, units, connected, requestQuit } = useStore();
  const [pinned, setPinned] = useState(false);
  const [tab, setTab] = useState<PanelTab>("top");
  const [tooltipReset, setTooltipReset] = useState(0);
  const bodyRef = useRef<HTMLDivElement>(null);
  const lastSize = useRef<{ width: number; height: number } | null>(null);
  const nav = panelNav(snapshot);
  const navKey = nav.join(" ");

  // The digits follow the strip on screen, which changes with the hardware.
  useEffect(() => {
    const strip = navKey.split(" ") as PanelTab[];
    const onKey = (event: KeyboardEvent) => {
      // A dialog that took Escape for itself has already marked it handled; closing the whole
      // panel as well would throw away the answer the dialog was waiting for.
      if (event.key === "Escape" && !event.defaultPrevented) void ipc.hidePanel();
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      const target = panelTabForKey(event.key, strip);
      if (target) setTab(target);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navKey]);

  // Left on a tab the hardware stopped justifying, the panel would show a heading over nothing.
  useEffect(() => {
    if (!navKey.split(" ").includes(tab)) setTab("top");
  }, [navKey, tab]);

  // The window follows the card only when the card's size really changed. This used to run
  // after every snapshot and send the same two numbers across the bridge once a second.
  useEffect(() => {
    const node = bodyRef.current;
    if (!node) return;
    let timer = 0;
    const measure = () => {
      timer = 0;
      const style = getComputedStyle(node);
      const px = (value: string) => Number.parseFloat(value) || 0;
      const size = panelWindowSize({
        scrollWidth: node.scrollWidth,
        scrollHeight: node.scrollHeight,
        borderX: node.offsetWidth - node.clientWidth,
        borderY: node.offsetHeight - node.clientHeight,
        marginX: px(style.marginLeft) + px(style.marginRight),
        marginY: px(style.marginTop) + px(style.marginBottom),
      });
      const last = lastSize.current;
      if (last && last.width === size.width && last.height === size.height) return;
      lastSize.current = size;
      ipc.setPanelSize(size.width, size.height);
    };
    // A tab switch or a row appearing is a burst of layout changes; it settles into one resize.
    const schedule = () => {
      if (!timer) timer = window.setTimeout(measure, 50);
    };
    measure();
    const observer = new ResizeObserver(schedule);
    observer.observe(node);
    return () => {
      observer.disconnect();
      if (timer) window.clearTimeout(timer);
    };
  }, []);

  // The panel is hidden, never unmounted, so whatever tooltip was up when it lost focus would
  // otherwise greet the next opening. Close them on the way out and again on the way in.
  useEffect(() => {
    const reset = () => {
      setTooltipReset((n) => n + 1);
      // A hidden window gets no pointerleave, so the held order would outlive the visit.
      pointerInside.current = false;
      setHeld(null);
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") reset();
    };
    window.addEventListener("blur", reset);
    document.addEventListener("visibilitychange", onVisibility);
    const stop = subscribe(() => listenEvent("panel-shown", reset));
    return () => {
      window.removeEventListener("blur", reset);
      document.removeEventListener("visibilitychange", onVisibility);
      stop();
    };
  }, []);

  // The same rules as the window's lists: the setting decides whether Windows' own processes
  // appear, pins come first, and the order holds while the pointer is over the rows so the ×
  // does not slide to another app between two samples.
  const [pins, togglePin] = usePins();
  const [held, setHeld] = useState<string[] | null>(null);
  const [menu, setMenu] = useState<AppMenuTarget | null>(null);
  const pointerInside = useRef(false);
  const ordered = useMemo(
    () => orderApps(forLists(snapshot?.apps ?? [], settings.showSystemAppsInLists), pins, "memory"),
    [snapshot, settings.showSystemAppsInLists, pins],
  );
  const latest = useRef(ordered);
  latest.current = ordered;
  const apps = useMemo(() => holdOrder(ordered, held).slice(0, 6), [ordered, held]);
  const closeMenu = useCallback(() => {
    setMenu(null);
    if (!pointerInside.current) setHeld(null);
  }, []);
  // A pin is the user's own doing: show it at the top at once, even while the order is held.
  const pinsKey = pins.join("\u000a");
  useEffect(() => {
    setHeld((current) => (current ? latest.current.map((a) => a.key) : null));
  }, [pinsKey]);

  const tiles = snapshot ? panelTiles(settings.panel.tiles, snapshot) : [];
  const shown: MetricKind[] = tab === "top" ? tiles : [tab];
  const chip = snapshot ? cleanChipName(snapshot.machine.chipName) : "";

  return (
    <TooltipReset.Provider value={tooltipReset}>
    <div className="panel" ref={bodyRef}>
      <header className="panel-head">
        <span className="brand">
          <span className="brand-dot" />
          Meerkat
        </span>
        <span className="panel-machine">
          {snapshot ? (
            <>
              {/* The whole name, marks stripped, cut by the box rather than by a word count. */}
              <Hint label={chip}>
                <span className="panel-chip">{chip}</span>
              </Hint>
              <span className="panel-total num">{formatBytes(snapshot.machine.totalMemoryBytes, units)}</span>
            </>
          ) : (
            t("state.noData")
          )}
        </span>
        <Hint label={t("panel.pin")}>
          <button
            className={`icon-btn ${pinned ? "on" : ""}`}
            aria-label={t("panel.pin")}
            onClick={() => {
              const next = !pinned;
              setPinned(next);
              ipc.setPanelPinned(next);
            }}
          >
            ◎
          </button>
        </Hint>
        <Hint label={t("panel.openWindow")}>
          <button className="icon-btn" aria-label={t("panel.openWindow")} onClick={() => void ipc.showWindow()}>
            ⤢
          </button>
        </Hint>
      </header>

      <div className={`panel-metrics ${settings.panel.layout}`}>
        {snapshot &&
          shown.map((metric) =>
            settings.panel.layout === "list" ? (
              <ListRow key={metric} metric={metric} snapshot={snapshot} t={t} units={units} />
            ) : (
              <Tile key={metric} metric={metric} snapshot={snapshot} t={t} units={units} />
            ),
          )}
      </div>

      {tab === "top" && (
        <div
          className="panel-apps"
          onPointerEnter={() => {
            pointerInside.current = true;
            setHeld((current) => current ?? latest.current.map((a) => a.key));
          }}
          onPointerLeave={() => {
            pointerInside.current = false;
            if (!menu) setHeld(null);
          }}
        >
          {apps.map((app) => {
            const label = appLabel(app, t);
            return (
              <Hint key={app.key} className="max-w-[320px] break-all" label={app.exePath || label}>
                <div
                  className={`panel-app ${pins.includes(app.key) ? "pinned" : ""}`}
                  tabIndex={0}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setMenu(menuTarget(app, label, event.currentTarget, event));
                  }}
                  onKeyDown={(event) => {
                    if (!isMenuKey(event)) return;
                    event.preventDefault();
                    setMenu(menuTarget(app, label, event.currentTarget));
                  }}
                >
                  <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
                  <span className="panel-app-name">{label}</span>
                  <span className="panel-app-mem num">{formatBytes(app.memoryBytes, units)}</span>
                  <span className="panel-app-cpu num">{formatPercent(cpuDisplay(app.cpuPercent, units), units.locale)}</span>
                  {canQuit(app) && (
                    <Hint label={t("action.quitApp")}>
                      <button
                        className="row-quit"
                        tabIndex={-1}
                        aria-label={t("action.quitApp")}
                        onClick={() => {
                          requestQuit(app);
                        }}
                      >
                        ×
                      </button>
                    </Hint>
                  )}
                </div>
              </Hint>
            );
          })}
          {apps.length === 0 && <p className="empty faint">{connected ? "" : t("state.noData")}</p>}
        </div>
      )}
      {menu && (
        <AppMenu
          target={menu}
          pinned={pins.includes(menu.app.key)}
          onTogglePin={togglePin}
          onClose={closeMenu}
        />
      )}

      <nav className="panel-tabs">
        {nav.map((id) => (
          <button key={id} className={tab === id ? "on" : ""} onClick={() => setTab(id)}>
            {id === "top" ? t("tab.overview") : t(`metric.${id}`)}
          </button>
        ))}
      </nav>
      <QuitDialog />
      <Toaster />
    </div>
    </TooltipReset.Provider>
  );
}

/**
 * The last minute of one metric. Percentages sit on a fixed 0–100 scale, where an idle machine
 * draws a line near the floor; rates scale to what the minute held, with a ceiling that only
 * comes down after a few calm samples so the line does not bounce.
 */
function Spark({ metric, snapshot, height, className, strokeWidth }: {
  metric: MetricKind;
  snapshot: Snapshot;
  height: number;
  className: string;
  strokeWidth: number;
}) {
  const values = panelSeries(metric, snapshot);
  const held = useHeldCeiling(windowPeak(values));
  const ceiling = SPARK_CEILING[metric] ?? held;
  return (
    <svg className={className} viewBox={`0 0 100 ${height}`} preserveAspectRatio="none">
      <polyline
        points={sparkPoints(values, 100, height, ceiling)}
        fill="none"
        stroke={HUES[metric]}
        strokeWidth={strokeWidth}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

function Tile({
  metric,
  snapshot,
  t,
  units,
}: {
  metric: MetricKind;
  snapshot: Snapshot;
  t: ReturnType<typeof useStore>["t"];
  units: ReturnType<typeof useStore>["units"];
}) {
  return (
    <div className="tile" style={{ ["--hue" as string]: HUES[metric] }}>
      <span className="tile-label">{t(`metric.${metric}`)}</span>
      <span className="tile-value num">{textFor(metric, snapshot, units)}</span>
      <Spark metric={metric} snapshot={snapshot} height={22} className="tile-spark" strokeWidth={1.2} />
    </div>
  );
}

function ListRow({
  metric,
  snapshot,
  t,
  units,
}: {
  metric: MetricKind;
  snapshot: Snapshot;
  t: ReturnType<typeof useStore>["t"];
  units: ReturnType<typeof useStore>["units"];
}) {
  return (
    <div className="list-row" style={{ ["--hue" as string]: HUES[metric] }}>
      <span className="list-label">{t(`metric.${metric}`)}</span>
      <span className="list-value num">{textFor(metric, snapshot, units)}</span>
      <Spark metric={metric} snapshot={snapshot} height={14} className="list-spark" strokeWidth={1} />
    </div>
  );
}

/** A reading the hardware did not give is a dash, never a zero that looks measured. */
function textFor(metric: MetricKind, snapshot: Snapshot, units: ReturnType<typeof useStore>["units"]): string {
  switch (metric) {
    case "cpu":
      return formatPercent(snapshot.cpu.totalPercent, units.locale);
    case "memory":
      return snapshot.memory.totalBytes
        ? `${formatPercent(headline("memory", snapshot), units.locale)} · ${formatBytes(snapshot.memory.inUseBytes, units)}`
        : "—";
    case "disk":
      return `↓${formatRateShort(snapshot.disk.readBytesPerSecond, units, "storage")} ↑${formatRateShort(snapshot.disk.writeBytesPerSecond, units, "storage")}`;
    case "network":
      return `↓${formatRateShort(snapshot.network.downloadBytesPerSecond, units, "network")} ↑${formatRateShort(snapshot.network.uploadBytesPerSecond, units, "network")}`;
    case "gpu":
      return snapshot.gpu.totalPercent === null ? "—" : formatPercent(snapshot.gpu.totalPercent, units.locale);
    case "battery":
      return snapshot.battery.hasBattery && snapshot.battery.chargePercent !== null
        ? formatPercent(snapshot.battery.chargePercent, units.locale)
        : "—";
    case "temperature":
      return snapshot.temperature.hottest !== null ? formatTemp(snapshot.temperature.hottest, units) : "—";
  }
}
