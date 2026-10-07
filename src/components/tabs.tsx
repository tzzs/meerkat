import { useEffect, useMemo, useState } from "react";
import {
  formatBytes,
  formatDuration,
  formatPercent,
  formatRate,
  formatTemp,
} from "../lib/format";
import { ipc } from "../lib/ipc";
import { sectionShown, useStore, visibleSections } from "../lib/store";
import type { AppInfo, HistorySeries, Insight, MetricKind } from "../lib/types";
import {
  AppTable,
  AvailabilityNote,
  Card,
  Chart,
  CoreGrid,
  Meter,
  Pill,
  Stat,
  type AppMetric,
} from "./ui";

export const RANGES: { seconds: number; key: "range.1h" | "range.12h" | "range.24h" | "range.7d" | "range.30d" }[] = [
  { seconds: 3600, key: "range.1h" },
  { seconds: 12 * 3600, key: "range.12h" },
  { seconds: 24 * 3600, key: "range.24h" },
  { seconds: 7 * 86400, key: "range.7d" },
  { seconds: 30 * 86400, key: "range.30d" },
];

const TONE: Record<MetricKind, string> = {
  cpu: "var(--m-cpu)",
  memory: "var(--m-memory)",
  disk: "var(--m-disk)",
  network: "var(--m-network)",
  gpu: "var(--m-gpu)",
  battery: "var(--m-battery)",
  temperature: "var(--m-temp)",
};

function useSeries(metric: string, rangeSeconds: number): HistorySeries | null {
  const [series, setSeries] = useState<HistorySeries | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => void ipc.historySeries(metric, rangeSeconds).then((s) => alive && setSeries(s));
    load();
    const timer = window.setInterval(load, 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [metric, rangeSeconds]);
  return series;
}

export function RangePicker({ value, onChange }: { value: number; onChange: (seconds: number) => void }) {
  const { t } = useStore();
  return (
    <div className="range-picker" role="tablist">
      {RANGES.map((range) => (
        <button
          key={range.seconds}
          role="tab"
          aria-selected={value === range.seconds}
          className={`chip ${value === range.seconds ? "on" : ""}`}
          onClick={() => onChange(range.seconds)}
        >
          {t(range.key)}
        </button>
      ))}
    </div>
  );
}

function HistoryChart({ metric, range }: { metric: string; range: number }) {
  const { units, t } = useStore();
  const series = useSeries(metric, range);
  const tone = TONE[metric as MetricKind] ?? "var(--accent)";
  const points = series?.points ?? [];
  const rangeLabel = RANGES.find((r) => r.seconds === range);
  if (points.length < 2) {
    return (
      <div className="chart-placeholder">
        <span>{points.length === 0 ? t("state.noData") : "…"}</span>
      </div>
    );
  }
  const isBytes = /diskRead|diskWrite|download|upload/.test(metric);
  const max = Math.max(...points.map((p) => p.value));
  const peak = isBytes
    ? formatRate(max, units, isBytes && /download|upload/.test(metric) ? "network" : "storage")
    : `${Math.round(max)}${metric === "temp" ? "°" : "%"}`;
  return (
    <div className="history">
      <Chart values={points.map((p) => p.value)} max={max} color={tone} height={110} />
      <div className="history-scale">
        <span className="faint">
          {t("label.utilisation").trim()} {peak}
        </span>
        <span className="faint">{rangeLabel ? t(rangeLabel.key) : ""}</span>
      </div>
    </div>
  );
}

function useApps(sort: AppMetric, hideSystem: boolean): AppInfo[] {
  const { snapshot, settings } = useStore();
  return useMemo(() => {
    const apps = snapshot?.apps ?? [];
    const pool = settings.showSystemAppsInLists || !hideSystem ? apps : apps.filter((a) => a.kind !== "system" && a.kind !== "service");
    const weight = (a: AppInfo) =>
      sort === "cpu" ? a.cpuPercent : sort === "disk" ? a.readBytesPerSecond + a.writeBytesPerSecond : a.memoryBytes;
    return [...pool].sort((a, b) => weight(b) - weight(a));
  }, [snapshot, sort, hideSystem, settings.showSystemAppsInLists]);
}

function useInsights(): Insight[] {
  const [list, setList] = useState<Insight[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => void ipc.insights().then((v) => alive && setList(v));
    load();
    const timer = window.setInterval(load, 15_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);
  return list;
}

export function OverviewTab({ onOpenTab }: { onOpenTab: (tab: string) => void }) {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "overview");
  const [range, setRange] = useState(3600);
  const [metric, setMetric] = useState<AppMetric>("memory");
  const apps = useApps(metric, false);
  const insights = useInsights().filter((i) => !i.resolved);
  if (!snapshot) return <Missing />;

  const cards: { metric: MetricKind; label: string; value: string; sub: string; open: string }[] = [
    {
      metric: "cpu",
      label: t("metric.cpu"),
      value: `${Math.round(snapshot.cpu.totalPercent)}%`,
      sub: `${snapshot.machine.logicalCores} ${t("label.cores")}`,
      open: "cpu",
    },
    {
      metric: "memory",
      label: t("metric.memory"),
      value: formatBytes(snapshot.memory.inUseBytes, units),
      sub: `${formatPercent(snapshot.memory.pressurePercent, units.locale)} ${t("label.pressure")}`,
      open: "memory",
    },
    {
      metric: "disk",
      label: t("metric.disk"),
      value: formatRate(snapshot.disk.writeBytesPerSecond, units, "storage"),
      sub: `${t("label.read")} ${formatBytes(snapshot.disk.readBytesToday, units)} · ${t("label.write")} ${formatBytes(snapshot.disk.writeBytesToday, units)}`,
      open: "disk",
    },
    {
      metric: "network",
      label: t("metric.network"),
      value: formatRate(snapshot.network.downloadBytesPerSecond, units, "network"),
      sub: `${t("label.upload")} ${formatRate(snapshot.network.uploadBytesPerSecond, units, "network")}`,
      open: "network",
    },
    {
      metric: "gpu",
      label: t("metric.gpu"),
      value: snapshot.gpu.totalPercent === null ? "—" : `${Math.round(snapshot.gpu.totalPercent)}%`,
      sub: snapshot.gpu.adapters[0]?.name ?? "",
      open: "gpu",
    },
    {
      metric: "battery",
      label: t("metric.battery"),
      value: snapshot.battery.hasBattery ? `${Math.round(snapshot.battery.chargePercent ?? 0)}%` : "—",
      sub: snapshot.battery.hasBattery
        ? snapshot.battery.isCharging
          ? t("label.charging")
          : `${t("label.timeRemaining")} ${snapshot.battery.remainingSeconds ? formatDuration(snapshot.battery.remainingSeconds, units.locale, { short: true }) : "—"}`
        : t("battery.noBattery"),
      open: "battery",
    },
    {
      metric: "temperature",
      label: t("metric.temperature"),
      value: snapshot.temperature.hottest ? formatTemp(snapshot.temperature.hottest, units) : "—",
      sub:
        snapshot.temperature.sensors.length > 0
          ? `${snapshot.temperature.sensors.length} ${t("label.sensors")}`
          : t(`state.${snapshot.temperature.availability === "needsElevation" ? "needsElevation" : "unavailable"}` as "state.unavailable"),
      open: "battery",
    },
  ];

  return (
    <div className="stack">
      <div className="metric-grid">
        {cards.map((card) => (
          <button
            key={card.metric}
            className={`metric-card ${card.metric}`}
            onClick={() => onOpenTab(card.open)}
            style={{ ["--tone" as string]: TONE[card.metric] }}
          >
            <span className="metric-top">
              <span className="metric-label">{card.label}</span>
              <span className="metric-value num">{card.value}</span>
            </span>
            <span className="metric-sub">{card.sub}</span>
            <Chart values={historyFor(snapshot, card.metric)} color={TONE[card.metric]} variant="line" height={30} />
          </button>
        ))}
      </div>

      {sectionShown(shown, "charts") && (
        <Card title={t("overview.title")} right={<RangePicker value={range} onChange={setRange} />}>
          <div className="two-col">
            <div>
              <h4 className="section-title">{t("metric.cpu")}</h4>
              <HistoryChart metric="cpu" range={range} />
            </div>
            <div>
              <h4 className="section-title">{t("metric.memory")}</h4>
              <HistoryChart metric="memory" range={range} />
            </div>
          </div>
        </Card>
      )}

      {sectionShown(shown, "topApps") && (
        <Card title={t("overview.topApps")}>
          <AppTable
            apps={apps}
            metric={metric}
            onMetricChange={setMetric}
            units={units}
            t={t}
            cap={9}
          />
        </Card>
      )}

      {sectionShown(shown, "worthALook") && (
        <Card title={t("overview.worthALook")} right={<Pill>{insights.length}</Pill>}>
          {insights.length === 0 ? (
            <p className="empty">{t("overview.worthALookEmpty")}</p>
          ) : (
            <InsightRows insights={insights} />
          )}
        </Card>
      )}

      {sectionShown(shown, "storage") && (
        <Card title={t("overview.volumes")}>
          <VolumeList />
        </Card>
      )}
    </div>
  );
}

function historyFor(snapshot: NonNullable<ReturnType<typeof useStore>["snapshot"]>, metric: MetricKind): number[] {
  switch (metric) {
    case "cpu":
      return snapshot.cpu.history;
    case "memory":
      return snapshot.memory.history;
    case "disk":
      return snapshot.disk.historyRead;
    case "network":
      return snapshot.network.historyDownload;
    case "gpu":
      return snapshot.gpu.history;
    case "battery":
      return snapshot.battery.history;
    case "temperature":
      return snapshot.temperature.history;
  }
}

export function CpuTab() {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "cpu");
  const [range, setRange] = useState(3600);
  const [metric, setMetric] = useState<AppMetric>("cpu");
  const apps = useApps("cpu", false);
  if (!snapshot) return <Missing />;
  const totalBusy = snapshot.cpu.userPercent + snapshot.cpu.privilegedPercent;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.cpu} title={t("cpu.title")}>
          <div className="hero-figure">
            <span className="huge num">{formatPercent(snapshot.cpu.totalPercent, units.locale)}</span>
            <span className="faint">{t("label.loadQueue")}: {snapshot.cpu.processorQueueLength ?? "—"}</span>
          </div>
          <Chart values={snapshot.cpu.history} max={100} color={TONE.cpu} height={80} />
        </Card>
        <Card title={t("label.userTime")} tone={TONE.cpu}>
          <div className="split-bar">
            <span style={{ flexGrow: snapshot.cpu.userPercent }} className="split user" />
            <span style={{ flexGrow: snapshot.cpu.privilegedPercent }} className="split sys" />
            <span style={{ flexGrow: Math.max(0, 100 - totalBusy) }} className="split idle" />
          </div>
          <div className="legend">
            <span><i className="swatch user" />{t("label.userTime")} {formatPercent(snapshot.cpu.userPercent, units.locale, 1)}</span>
            <span><i className="swatch sys" />{t("label.privilegedTime")} {formatPercent(snapshot.cpu.privilegedPercent, units.locale, 1)}</span>
          </div>
          <p className="hint faint">{t("cpu.queueHelp")}</p>
        </Card>
      </div>

      {sectionShown(shown, "perCore") && (
        <Card title={`${t("label.perCore")} · ${snapshot.machine.logicalCores}`}>
          <CoreGrid values={snapshot.cpu.corePercents} units={units} />
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric="cpu" range={range} />
        </Card>
      )}

      {sectionShown(shown, "apps") && (
        <Card title={t("overview.topApps")}>
          <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
        </Card>
      )}
    </div>
  );
}

export function MemoryTab({ search }: { search?: string }) {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "memory");
  const [range, setRange] = useState(3600);
  const [metric, setMetric] = useState<AppMetric>("memory");
  const apps = useApps("memory", false);
  if (!snapshot) return <Missing />;
  const m = snapshot.memory;
  const rows: [string, number, number][] = [
    [t("label.inUse"), m.inUseBytes, m.totalBytes],
    [t("label.available"), m.availableBytes, m.totalBytes],
    [t("label.cached"), m.cachedBytes, m.totalBytes],
    [t("label.committed"), m.committedBytes, m.commitLimitBytes],
    [t("label.pagefile"), m.pagefileUsedBytes, m.pagefileTotalBytes],
    [t("label.pagedPool"), m.pagedPoolBytes, m.totalBytes],
    [t("label.nonpagedPool"), m.nonpagedPoolBytes, m.totalBytes],
  ];

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.memory} title={t("memory.title")}>
          <div className="hero-figure">
            <span className="huge num">{formatBytes(m.inUseBytes, units)}</span>
            <span className="faint">{t("label.ofTotal", { total: formatBytes(m.totalBytes, units) })}</span>
          </div>
          <Meter ratio={m.inUseBytes / Math.max(1, m.totalBytes)} color={TONE.memory} />
          <p className="hint faint">{t("memory.pressureHelp")}</p>
        </Card>
        <Card title={t("label.pressure")} tone={TONE.memory}>
          <div className="hero-figure">
            <span className="huge num" style={{ fontSize: 30 }}>
              {formatPercent(m.pressurePercent, units.locale)}
            </span>
          </div>
          <Chart values={m.history} max={100} color={TONE.memory} height={72} />
          <div className="legend">
            <span>
              {t("label.committed")} {formatPercent((m.committedBytes / Math.max(1, m.commitLimitBytes)) * 100, units.locale)}
            </span>
            <span>
              {formatBytes(m.commitLimitBytes, units)} {t("label.commitLimit")}
            </span>
          </div>
        </Card>
      </div>

      {sectionShown(shown, "breakdown") && (
        <Card title={t("memory.title")}>
          <div className="breakdown">
            {rows.map(([label, value, limit]) => (
              <div key={label} className="break-row">
                <span className="break-label">{label}</span>
                <Meter ratio={value / Math.max(1, limit)} color={TONE.memory} />
                <span className="break-value num">{formatBytes(value, units)}</span>
              </div>
            ))}
          </div>
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric="memory" range={range} />
        </Card>
      )}

      {sectionShown(shown, "apps") && (
        <Card title={t("overview.topApps")}>
          <AppTable
            apps={apps}
            metric={metric}
            onMetricChange={setMetric}
            units={units}
            t={t}
            seedQuery={search}
          />
        </Card>
      )}
    </div>
  );
}

export function DiskTab() {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "disk");
  const [range, setRange] = useState(3600);
  const [metric, setMetric] = useState<AppMetric>("disk");
  const apps = useApps("disk", false);
  if (!snapshot) return <Missing />;
  const d = snapshot.disk;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.disk} title={t("disk.title")}>
          <div className="rate-pair">
            <div>
              <span className="rate-label">{t("label.read")}</span>
              <span className="huge num">{formatRate(d.readBytesPerSecond, units, "storage")}</span>
              <Chart values={d.historyRead} color={TONE.disk} height={54} />
            </div>
            <div>
              <span className="rate-label">{t("label.write")}</span>
              <span className="huge num">{formatRate(d.writeBytesPerSecond, units, "storage")}</span>
              <Chart values={d.historyWrite} color={TONE.disk} variant="bars" height={54} />
            </div>
          </div>
        </Card>
        <Card title={t("disk.title")} tone={TONE.disk}>
          <Stat label={t("label.readToday")} value={formatBytes(d.readBytesToday, units)} sub={t("label.sinceMidnight")} />
          <Stat label={t("label.writeToday")} value={formatBytes(d.writeBytesToday, units)} sub={t("label.sinceMidnight")} />
          <p className="hint faint">{t("disk.dailyHelp")}</p>
          <AvailabilityNote state={d.availability} t={t} />
        </Card>
      </div>

      {sectionShown(shown, "volumes") && (
        <Card title={t("overview.volumes")}>
          <VolumeList />
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric="diskWrite" range={range} />
        </Card>
      )}

      {sectionShown(shown, "apps") && (
        <Card title={t("overview.topApps")}>
          <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
        </Card>
      )}
    </div>
  );
}

export function NetworkTab() {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "network");
  const [range, setRange] = useState(3600);
  const [metric, setMetric] = useState<AppMetric>("connections");
  const apps = useApps("connections", false);
  if (!snapshot) return <Missing />;
  const n = snapshot.network;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.network} title={t("network.title")}>
          <div className="rate-pair">
            <div>
              <span className="rate-label">{t("label.download")}</span>
              <span className="huge num">{formatRate(n.downloadBytesPerSecond, units, "network")}</span>
              <Chart values={n.historyDownload} color={TONE.network} height={54} />
            </div>
            <div>
              <span className="rate-label">{t("label.upload")}</span>
              <span className="huge num">{formatRate(n.uploadBytesPerSecond, units, "network")}</span>
              <Chart values={n.historyUpload} color={TONE.network} variant="bars" height={54} />
            </div>
          </div>
        </Card>
        <Card title={t("network.title")} tone={TONE.network}>
          <Stat label={t("label.downloadToday")} value={formatBytes(n.downloadBytesToday, units)} sub={t("label.sinceMidnight")} />
          <Stat label={t("label.uploadToday")} value={formatBytes(n.uploadBytesToday, units)} sub={t("label.sinceMidnight")} />
          <Stat label={t("label.connections")} value={String(n.connections)} />
        </Card>
      </div>

      {sectionShown(shown, "adapters") && (
        <Card title={t("overview.adapters")}>
          <div className="breakdown">
            {n.adapters
              .filter((a) => !a.isLoopback)
              .map((a) => (
                <div key={a.name} className="break-row">
                  <span className="break-label">{a.name}</span>
                  <Pill tone={a.isUp ? "" : "faint"}>{a.isUp ? "up" : "down"}</Pill>
                  <span className="break-value num">
                    {formatRate(a.downloadBytesPerSecond, units, "network")}
                  </span>
                </div>
              ))}
          </div>
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric="download" range={range} />
        </Card>
      )}

      {sectionShown(shown, "connections") && (
        <Card title={t("overview.topApps")} right={<p className="hint faint">{t("network.perAppHelp")}</p>}>
          <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
        </Card>
      )}
    </div>
  );
}

export function GpuTab() {
  const { snapshot, settings, units, t } = useStore();
  const shown = visibleSections(settings, "gpu");
  const [range, setRange] = useState(3600);
  if (!snapshot) return <Missing />;
  const g = snapshot.gpu;
  const appFor = (pid: number) => snapshot.apps.find((a) => a.pids.includes(pid));

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.gpu} title={t("gpu.title")}>
          <div className="hero-figure">
            <span className="huge num">{g.totalPercent === null ? "—" : formatPercent(g.totalPercent, units.locale)}</span>
            <span className="faint">{g.adapters.length} {g.adapters.length === 1 ? "adapter" : "adapters"}</span>
          </div>
          <Chart values={g.history} max={100} color={TONE.gpu} height={80} />
          <AvailabilityNote state={g.availability} t={t} />
        </Card>
        <Card title={t("label.dedicated")} tone={TONE.gpu}>
          {g.adapters.map((adapter) => (
            <div key={adapter.index} className="adapter">
              <strong>{adapter.name}</strong>
              <div className="legend">
                <span>
                  {adapter.dedicatedUsedBytes && adapter.dedicatedTotalBytes
                    ? `${formatBytes(adapter.dedicatedUsedBytes, units)} / ${formatBytes(adapter.dedicatedTotalBytes, units)}`
                    : // NVIDIA leaves DXGI's CurrentUsage at zero, so an unreported figure
                      // shows as unavailable rather than as a confident 0.
                      `${t("state.unavailable")} / ${adapter.dedicatedTotalBytes ? formatBytes(adapter.dedicatedTotalBytes, units) : "—"}`}
                </span>
                {adapter.sharedUsedBytes ? <span>{t("label.shared")} {formatBytes(adapter.sharedUsedBytes, units)}</span> : null}
              </div>
              {adapter.dedicatedTotalBytes ? (
                <Meter ratio={(adapter.dedicatedUsedBytes ?? 0) / adapter.dedicatedTotalBytes} color={TONE.gpu} />
              ) : null}
            </div>
          ))}
        </Card>
      </div>

      {sectionShown(shown, "apps") && (
        <Card title={t("overview.topApps")}>
          <div className="rows">
            {g.perProcess.slice(0, 20).map((row) => {
              const owner = appFor(row.pid);
              const name = owner?.name?.trim() || `pid ${row.pid}`;
              return (
                <div key={row.pid} className="row">
                  <span className="mono-avatar">{(name[0] ?? "?").toUpperCase()}</span>
                  <div className="row-main">
                    <span className="row-name">{name}</span>
                    <span className="row-sub faint">{owner ? `${t("metric.gpu")} · ${row.pid}` : "—"}</span>
                  </div>
                  <span className="row-value num">{formatPercent(row.percent, units.locale, 1)}</span>
                </div>
              );
            })}
            {g.perProcess.length === 0 && <p className="empty">{t("state.noData")}</p>}
          </div>
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric="gpu" range={range} />
        </Card>
      )}
    </div>
  );
}

export function BatteryTab() {
  const { snapshot, settings, units, t } = useStore();
  const [range, setRange] = useState(3600);
  const shown = visibleSections(settings, "battery");
  if (!snapshot) return <Missing />;
  const b = snapshot.battery;
  const temp = snapshot.temperature;

  return (
    <div className="stack">
      <div className="hero-row">
        {b.hasBattery ? (
          <Card className="hero" tone={TONE.battery} title={t("battery.title")}>
            <div className="hero-figure">
              <span className="huge num">{formatPercent(b.chargePercent ?? 0, units.locale)}</span>
              <span className="faint">{b.isCharging ? t("label.charging") : b.onAcPower ? t("label.onAc") : t("label.timeRemaining")}</span>
            </div>
            <Meter ratio={(b.chargePercent ?? 0) / 100} color={b.isCritical ? "var(--m-danger)" : TONE.battery} />
            <div className="legend">
              <span>{t("label.timeRemaining")} {b.remainingSeconds ? formatDuration(b.remainingSeconds, units.locale) : "—"}</span>
              {b.powerWatts !== null && <span>{t("label.powerDraw")} {b.powerWatts?.toFixed(1)} W</span>}
            </div>
          </Card>
        ) : (
          <Card className="hero" tone={TONE.battery} title={t("battery.title")}>
            <p className="empty">{t("battery.noBattery")}</p>
          </Card>
        )}
        <Card title={t("label.health")} tone={TONE.battery}>
          {b.healthPercent !== null && <Stat label={t("label.health")} value={formatPercent(b.healthPercent, units.locale)} />}
          {b.cycleCount !== null && <Stat label={t("label.cycles")} value={String(b.cycleCount)} />}
          {b.designCapacityWh !== null && <Stat label={t("label.designCapacity")} value={`${b.designCapacityWh.toFixed(0)} Wh`} />}
          {b.fullChargeCapacityWh !== null && (
            <Stat label={t("label.fullCharge")} value={`${b.fullChargeCapacityWh.toFixed(0)} Wh`} />
          )}
          <AvailabilityNote state={b.source} t={t} />
        </Card>
      </div>

      {sectionShown(shown, "power") && (
        <Card title={t("temperature.title")} tone={TONE.temperature}>
          <div className="hero-figure">
            <span className="huge num" style={{ color: TONE.temperature }}>
              {temp.hottest ? formatTemp(temp.hottest, units) : "—"}
            </span>
            <span className="faint">{t("label.hottest")}</span>
          </div>
          <div className="breakdown">
            {temp.sensors.map((s) => (
              <div key={s.id} className="break-row">
                <span className="break-label">{s.label}</span>
                <Meter ratio={Math.min(1, s.celsius / 110)} color={TONE.temperature} />
                <span className="break-value num">{formatTemp(s.celsius, units)}</span>
              </div>
            ))}
          </div>
          <AvailabilityNote state={temp.availability} t={t} />
        </Card>
      )}

      {sectionShown(shown, "history") && (
        <Card right={<RangePicker value={range} onChange={setRange} />}>
          <HistoryChart metric={b.hasBattery ? "battery" : "temp"} range={range} />
        </Card>
      )}
    </div>
  );
}

function VolumeList() {
  const { snapshot, units, t } = useStore();
  if (!snapshot) return null;
  return (
    <div className="breakdown">
      {snapshot.disk.volumes.map((volume) => (
        <div key={volume.id} className="volume">
          <div className="volume-head">
            <strong>{volume.mountPoint}</strong>
            <span className="faint">
              {volume.name} · {volume.fileSystem}
              {volume.isSolidState ? " · SSD" : ""}
              {volume.isRemovable ? " · removable" : ""}
            </span>
            <span className="num volume-value">
              {formatBytes(volume.freeBytes, units)} {t("label.free")}
            </span>
          </div>
          <Meter ratio={volume.usedBytes / Math.max(1, volume.totalBytes)} color={TONE.disk} />
          <div className="legend">
            <span>{formatBytes(volume.usedBytes, units)} / {formatBytes(volume.totalBytes, units)}</span>
            <span>
              {formatRate(volume.readBytesPerSecond, units, "storage")} {formatRate(volume.writeBytesPerSecond, units, "storage")}
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}

function InsightRows({ insights }: { insights: Insight[] }) {
  const { t, units } = useStore();
  const text = (insight: Insight) => {
    switch (insight.kind) {
      case "highCpu":
        return `${insight.appName} ${t("settings.alertCpu").toLowerCase()} ${formatPercent(insight.value, units.locale)}`;
      case "growingMemory":
        return `${insight.appName} +${formatBytes(insight.value, units)}`;
      case "heavyDisk":
        return `${insight.appName} ${formatRate(insight.value, units, "storage")}`;
      case "heavyNetwork":
        return `${insight.appName} ${formatRate(insight.value, units, "network")}`;
      case "heat":
        return `${t("label.hottest")} ${formatTemp(insight.value, units)}`;
    }
  };
  return (
    <div className="rows">
      {insights.map((insight) => (
        <div key={insight.id} className="row">
          <span className={`kind-dot ${insight.kind}`} />
          <div className="row-main">
            <span className="row-name">{text(insight)}</span>
            <span className="row-sub faint">
              {new Date(insight.atMs).toLocaleString(units.locale)}
              {insight.repeatCount > 1 ? ` · ${insight.repeatCount}×` : ""}
            </span>
          </div>
          <button
            className="row-quit"
            onClick={() => {
              ipc.dismissInsight(insight.id);
            }}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

function Missing() {
  const { t } = useStore();
  return <p className="empty">{t("state.noData")}</p>;
}
