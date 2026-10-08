import { useEffect, useState, type ReactNode } from "react";
import { ipc } from "../lib/ipc";
import { useStore, fallbackSettings } from "../lib/store";
import { formatBytes, formatRate, formatTemp } from "../lib/format";
import { METRIC_ORDER, powerTabMode, type MetricKind, type Settings, type TrayReadout, type TrayStyle } from "../lib/types";
import { TAB_LABEL } from "../i18n";
import type { Key } from "../i18n/en";

type Page = "general" | "layout" | "about";

const TRAY_METRICS: Record<MetricKind, Key> = {
  cpu: "metric.cpu",
  memory: "metric.memory",
  disk: "metric.disk",
  network: "metric.network",
  gpu: "metric.gpu",
  battery: "metric.battery",
  temperature: "metric.temperature",
};

export function SettingsPages({ page, setPage }: { page: Page; setPage: (p: Page | null) => void }) {
  const { settings, save, t } = useStore();
  const pages: [Page, string][] = [
    ["general", t("settings.general")],
    ["layout", t("settings.layout")],
    ["about", t("settings.about")],
  ];
  return (
    <div className="settings">
      <aside className="settings-nav">
        {pages.map(([id, label]) => (
          <button key={id} className={page === id ? "on" : ""} onClick={() => setPage(id)}>
            {label}
          </button>
        ))}
      </aside>
      <div className="settings-body">
        {page === "general" && <General settings={settings} save={save} />}
        {page === "layout" && <Layout settings={settings} save={save} />}
        {page === "about" && <About />}
        <button className="reset" onClick={() => save({ ...fallbackSettings, tray: settings.tray })}>
          {t("action.reset")}
        </button>
      </div>
    </div>
  );
}

function Row({ label, children, hint }: { label: string; children?: ReactNode; hint?: string }) {
  return (
    <label className="row-setting">
      <span className="row-label">{label}</span>
      <span className="row-control">{children}</span>
      {hint && <span className="row-hint faint">{hint}</span>}
    </label>
  );
}

function select<T extends string>(value: T, options: [T, string][], onChange: (v: T) => void) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value as T)}>
      {options.map(([v, label]) => (
        <option key={v} value={v}>
          {label}
        </option>
      ))}
    </select>
  );
}

function toggle(value: boolean, onChange: (v: boolean) => void) {
  return <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} />;
}

function General({ settings, save }: { settings: Settings; save: (s: Settings) => void }) {
  const { t, snapshot, units } = useStore();
  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => save({ ...settings, [key]: value });
  const sample = snapshot;

  return (
    <div className="settings-group">
      <h4 className="section-title">{t("settings.units")}</h4>
      <Row label={t("settings.temperatureUnit")} hint={sample?.temperature.hottest ? formatTemp(sample.temperature.hottest, { ...units, temperature: settings.temperatureUnit }) : undefined}>
        {select(settings.temperatureUnit, [["celsius", "°C"], ["fahrenheit", "°F"]], (v) => set("temperatureUnit", v))}
      </Row>
      <Row
        label={t("settings.networkUnit")}
        hint={sample ? formatRate(sample.network.downloadBytesPerSecond, { ...units, network: settings.networkUnit }, "network") : undefined}
      >
        {select(settings.networkUnit, [["bytes", t("settings.bytes")], ["bits", t("settings.bits")]], (v) => set("networkUnit", v))}
      </Row>
      <Row
        label={t("settings.storageUnit")}
        hint={sample ? formatBytes(sample.memory.inUseBytes, { ...units, storage: settings.storageUnit }) : undefined}
      >
        {select(settings.storageUnit, [["decimal", t("settings.decimal")], ["binary", t("settings.binary")]], (v) => set("storageUnit", v))}
      </Row>
      <Row label={t("settings.cpuScale")}>
        {select(
          settings.cpuScale,
          [
            ["perMachine", t("settings.cpuPerMachine")],
            ["perCore", t("settings.cpuPerCore")],
          ],
          (v) => set("cpuScale", v),
        )}
      </Row>

      <h4 className="section-title">{t("app.name")}</h4>
      <Row label={t("settings.language")}>
        {select(
          settings.language,
          [
            ["auto", t("settings.languageAuto")],
            ["en", "English"],
            ["zhHans", "简体中文"],
          ],
          (v) => set("language", v),
        )}
      </Row>
      <Row label={t("settings.theme")}>
        {select(
          settings.theme,
          [
            ["auto", t("settings.themeAuto")],
            ["light", t("settings.themeLight")],
            ["dark", t("settings.themeDark")],
          ],
          (v) => set("theme", v),
        )}
      </Row>
      <Row label={t("settings.showSystemApps")}>{toggle(settings.showSystemAppsInLists, (v) => set("showSystemAppsInLists", v))}</Row>
      <Row label={t("settings.openWindowOn")}>
        <select value={settings.openWindowOn} onChange={(e) => set("openWindowOn", e.target.value)}>
          <option value="overview">{t("tab.overview")}</option>
          <option value="last">{t("settings.openWindowLast")}</option>
          {settings.window.tabs.filter((x) => x.visible).map((x) => (
            <option key={x.id} value={x.id}>
              {x.id}
            </option>
          ))}
        </select>
      </Row>
      <Row label={t("settings.startAtLogin")}>{toggle(settings.startAtLogin, (v) => set("startAtLogin", v))}</Row>
      <Row label={t("settings.showInTaskbar")}>{toggle(settings.showInTaskbar, (v) => set("showInTaskbar", v))}</Row>

      <h4 className="section-title">{t("settings.alerts")}</h4>
      <Row label={t("settings.alertsEnabled")}>
        {toggle(settings.alerts.enabled, (v) => save({ ...settings, alerts: { ...settings.alerts, enabled: v } }))}
      </Row>
      <Row label={t("settings.alertCpu")}>
        <NumberBox
          value={settings.alerts.cpuPercent}
          suffix="%"
          onChange={(v) => save({ ...settings, alerts: { ...settings.alerts, cpuPercent: v } })}
        />
      </Row>
      <Row label={t("settings.alertMemory")}>
        <NumberBox
          value={Math.round(settings.alerts.memoryGrowBytes / 1048576)}
          suffix="MB"
          onChange={(v) => save({ ...settings, alerts: { ...settings.alerts, memoryGrowBytes: v * 1048576 } })}
        />
      </Row>
      <Row label={t("settings.alertDisk")}>
        <NumberBox
          value={Math.round(settings.alerts.diskBytesPerSecond / 1048576)}
          suffix="MB/s"
          onChange={(v) => save({ ...settings, alerts: { ...settings.alerts, diskBytesPerSecond: v * 1048576 } })}
        />
      </Row>
      <Row label={t("settings.alertNetwork")}>
        <NumberBox
          value={Math.round(settings.alerts.networkBytesPerSecond / 1048576)}
          suffix="MB/s"
          onChange={(v) => save({ ...settings, alerts: { ...settings.alerts, networkBytesPerSecond: v * 1048576 } })}
        />
      </Row>
      <Row label={t("settings.alertSustained")}>
        <NumberBox
          value={settings.alerts.sustainedSeconds}
          suffix="s"
          onChange={(v) => save({ ...settings, alerts: { ...settings.alerts, sustainedSeconds: v } })}
        />
      </Row>
    </div>
  );
}

function NumberBox({ value, suffix, onChange }: { value: number; suffix: string; onChange: (v: number) => void }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  return (
    <span className="number-box">
      <input
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const parsed = Number(text);
          if (Number.isFinite(parsed)) onChange(parsed);
        }}
      />
      <span className="faint">{suffix}</span>
    </span>
  );
}

function Layout({ settings, save }: { settings: Settings; save: (s: Settings) => void }) {
  const { t, snapshot, units } = useStore();

  const updateTray = (next: TrayReadout[]) => save({ ...settings, tray: next });

  const move = (index: number, delta: number) => {
    const list = [...settings.tray];
    const target = index + delta;
    if (target < 0 || target >= list.length) return;
    [list[index], list[target]] = [list[target], list[index]];
    updateTray(list);
  };

  const addReadout = (metric: MetricKind) => {
    const readout: TrayReadout = {
      id: `${metric}-${settings.tray.length}`,
      metric,
      style: "value",
      showIcon: false,
      showCaption: false,
      invert: false,
      enabled: true,
    };
    updateTray([...settings.tray, readout]);
  };

  const usedMetrics = new Set(settings.tray.map((r) => r.metric));

  return (
    <div className="settings-group">
      <h4 className="section-title">{t("settings.tray")}</h4>
      <p className="hint faint">{t("settings.trayHelp")}</p>
      {settings.tray.map((readout, index) => (
        <div className="tray-row" key={readout.id}>
          <input
            type="checkbox"
            checked={readout.enabled}
            onChange={(e) =>
              updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, enabled: e.target.checked } : r)))
            }
          />
          <span className="tray-name">{t(TRAY_METRICS[readout.metric] as "metric.cpu")}</span>
          {select(
            readout.style,
            [
              ["value", t("settings.styleValue")],
              ["graph", t("settings.styleGraph")],
              ["both", t("settings.styleBoth")],
            ] as [TrayStyle, string][],
            (v) => updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, style: v } : r))),
          )}
          {readout.metric === "memory" && (
            <label className="inline-check">
              <input
                type="checkbox"
                checked={readout.invert}
                onChange={(e) =>
                  updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, invert: e.target.checked } : r)))
                }
              />
              {t("label.available")}
            </label>
          )}
          <span className="tray-preview num">{preview(readout, snapshot, units)}</span>
          <span className="spacer" />
          <button onClick={() => move(index, -1)} aria-label={t("settings.moveUp")}>
            ↑
          </button>
          <button onClick={() => move(index, 1)} aria-label={t("settings.moveDown")}>
            ↓
          </button>
          <button
            onClick={() => updateTray(settings.tray.filter((r) => r.id !== readout.id))}
            aria-label={t("action.cancel")}
          >
            ×
          </button>
        </div>
      ))}
      <div className="add-row">
        <span>{t("settings.addReadout")}</span>
        {METRIC_ORDER.filter((m) => !usedMetrics.has(m) && (m !== "battery" || snapshot?.battery.hasBattery)).map((metric) => (
          <button key={metric} className="chip" onClick={() => addReadout(metric)}>
            {t(TRAY_METRICS[metric] as "metric.cpu")}
          </button>
        ))}
      </div>

      <h4 className="section-title">{t("settings.panel")}</h4>
      <Row label={t("settings.panelLayout")}>
        {select(
          settings.panel.layout,
          [
            ["tiles", t("settings.panelTiles")],
            ["list", t("settings.panelList")],
          ],
          (v) => save({ ...settings, panel: { ...settings.panel, layout: v } }),
        )}
      </Row>
      <div className="tile-row">
        {settings.panel.tiles.map((tile) => (
          <button
            key={tile.id}
            className={`chip ${tile.visible ? "on" : ""}`}
            onClick={() =>
              save({
                ...settings,
                panel: {
                  ...settings.panel,
                  tiles: settings.panel.tiles.map((x) => (x.id === tile.id ? { ...x, visible: !x.visible } : x)),
                },
              })
            }
          >
            {t(TRAY_METRICS[tile.id as MetricKind] ?? "metric.cpu")}
          </button>
        ))}
      </div>

      <h4 className="section-title">{t("settings.windowTabs")}</h4>
      {settings.window.tabs.map((tab, index) => {
        // The power tab is not a choice on hardware that has nothing to put in it.
        const mode = powerTabMode(snapshot);
        const offHardware = tab.id === "battery" && mode === "hidden";
        const label = tab.id === "battery" && mode === "thermal" ? "metric.temperature" : TAB_LABEL[tab.id];
        return (
        <div className="tray-row" key={tab.id}>
          <input
            type="checkbox"
            checked={tab.visible}
            disabled={offHardware}
            onChange={(e) =>
              save({
                ...settings,
                window: {
                  tabs: settings.window.tabs.map((x, i) => (i === index ? { ...x, visible: e.target.checked } : x)),
                },
              })
            }
          />
          <span className="tray-name">{t(label)}</span>
          {offHardware && <span className="faint">{t("settings.noPowerHardware")}</span>}
          <span className="spacer" />
          <button
            onClick={() => {
              const list = [...settings.window.tabs];
              const target = index - 1;
              if (target < 0) return;
              [list[index], list[target]] = [list[target], list[index]];
              save({ ...settings, window: { tabs: list } });
            }}
          >
            ↑
          </button>
          <button
            onClick={() => {
              const list = [...settings.window.tabs];
              const target = index + 1;
              if (target >= list.length) return;
              [list[index], list[target]] = [list[target], list[index]];
              save({ ...settings, window: { tabs: list } });
            }}
          >
            ↓
          </button>
        </div>
        );
      })}
    </div>
  );
}

function preview(
  readout: TrayReadout,
  snapshot: ReturnType<typeof useStore>["snapshot"],
  units: ReturnType<typeof useStore>["units"],
): string {
  if (!snapshot) return "—";
  switch (readout.metric) {
    case "cpu":
      return `${Math.round(snapshot.cpu.totalPercent)}%`;
    case "memory": {
      const ratio = snapshot.memory.inUseBytes / Math.max(1, snapshot.memory.totalBytes);
      return readout.invert ? `${Math.round((1 - ratio) * 100)}%` : `${Math.round(ratio * 100)}%`;
    }
    case "disk":
      return formatRate(snapshot.disk.readBytesPerSecond + snapshot.disk.writeBytesPerSecond, units, "storage");
    case "network":
      return formatRate(snapshot.network.downloadBytesPerSecond, units, "network");
    case "gpu":
      return `${Math.round(snapshot.gpu.totalPercent ?? 0)}%`;
    case "battery":
      return `${Math.round(snapshot.battery.chargePercent ?? 0)}%`;
    case "temperature":
      return snapshot.temperature.hottest ? `${Math.round(snapshot.temperature.hottest)}°` : "—";
  }
}

function About() {
  const { t, snapshot, settings } = useStore();
  const [version, setVersion] = useState("…");
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    void ipc.version().then(setVersion);
  }, []);

  return (
    <div className="settings-group">
      <Row label={t("settings.aboutVersion")}>
        <span className="num">{version}</span>
      </Row>
      <Row label={t("settings.aboutSampler")}>
        <span className="num">{snapshot ? `${(snapshot.samplerCostMicroseconds / 1000).toFixed(1)} ms` : "—"}</span>
      </Row>
      <Row label={t("settings.privacy")} />
      <button
        className="primary"
        onClick={async () => {
          const report = await ipc.copyReport();
          await navigator.clipboard.writeText(report).catch(() => undefined);
          setCopied(true);
          window.setTimeout(() => setCopied(false), 2000);
        }}
      >
        {copied ? t("action.done") : t("settings.aboutCopyReport")}
      </button>
      <p className="hint faint">{t("settings.aboutReportHelp")}</p>
      <p className="hint faint">{JSON.stringify(settings.alerts)}</p>
    </div>
  );
}
