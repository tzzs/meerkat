import { useCallback, useEffect, useMemo, useState } from "react";
import {
  formatBytes,
  formatDuration,
  formatLinkSpeed,
  formatPercent,
  formatRate,
  formatTemp,
} from "../lib/format";
import { ipc } from "../lib/ipc";
import { sectionShown, useStore, visibleSections } from "../lib/store";
import type { AppInfo, HistorySeries, Insight, MetricKind } from "../lib/types";
import { appLabel, listedApps } from "../lib/apps";
import type { Translate } from "../i18n";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import {
  AppIcon,
  AppTable,
  AvailabilityNote,
  Card,
  Chart,
  CoreGrid,
  Meter,
  Pill,
  Stat,
  Tween,
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
    <ToggleGroup
      type="single"
      value={String(value)}
      onValueChange={(next) => {
        if (next) onChange(Number(next));
      }}
      aria-label={t("label.range")}
    >
      {RANGES.map((range) => (
        <ToggleChip key={range.seconds} value={String(range.seconds)}>
          {t(range.key)}
        </ToggleChip>
      ))}
    </ToggleGroup>
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
          {t("label.peak")} {peak}
        </span>
        <span className="faint">{rangeLabel ? t(rangeLabel.key) : ""}</span>
      </div>
    </div>
  );
}

/**
 * The apps a list shows, heaviest first. Every list honours "show Windows' own processes":
 * it used to be passed a flag that every caller set to false, so the setting did nothing, which
 * mattered little while Windows' processes read as zero and matters now that they are measured.
 */
function useApps(sort: AppMetric): AppInfo[] {
  const { snapshot, settings } = useStore();
  return useMemo(() => {
    const pool = listedApps(snapshot?.apps ?? [], settings.showSystemAppsInLists);
    const weight = (a: AppInfo) =>
      sort === "cpu" ? a.cpuPercent : sort === "disk" ? a.readBytesPerSecond + a.writeBytesPerSecond : a.memoryBytes;
    return [...pool].sort((a, b) => weight(b) - weight(a));
  }, [snapshot, sort, settings.showSystemAppsInLists]);
}

/**
 * Per-tab UI state that survives leaving the tab.
 *
 * Tabs unmount on purpose: eight tabs each re-rendering on every sample would cost far more
 * than the memory they would save. The price is that anything a tab keeps in useState is gone
 * when you come back, and the range picker is exactly what people notice — choose "last 7
 * days" on CPU, glance at Memory, return, and it is an hour again. This keeps that state in a
 * map keyed by tab and field, which lives exactly as long as the session does.
 */
const tabMemory = new Map<string, unknown>();

function useTabState<T>(tab: string, field: string, initial: T): [T, (next: T) => void] {
  const id = `${tab}.${field}`;
  const [value, setValue] = useState<T>(() => (tabMemory.has(id) ? (tabMemory.get(id) as T) : initial));
  const set = useCallback(
    (next: T) => {
      tabMemory.set(id, next);
      setValue(next);
    },
    [id],
  );
  return [value, set];
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
  const [range, setRange] = useTabState("overview", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("overview", "metric", "memory");
  const apps = useApps(metric);
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
          : snapshot.temperature.availability === "needsElevation"
            ? t("state.needsElevation")
            : snapshot.temperature.availability === "partial"
              ? t("state.partial")
              : t("state.unavailable"),
      open: "battery",
    },
  ];

  return (
    <div className="stack">
      <div className="metric-grid">
        {cards
          .filter((card) => card.metric !== "battery" || snapshot.battery.hasBattery)
          .map((card) => (
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
          {/* The list is a log of things that are still true, not a feed of events, and that
              distinction is the whole reason it confused the user who wrote this comment. */}
          <p className="hint faint">
            {t("overview.worthALookHelp", {
              seconds: formatDuration(settings.alerts.sustainedSeconds, units.locale, { short: true }),
            })}
          </p>
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
  const [range, setRange] = useTabState("cpu", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("cpu", "metric", "cpu");
  const apps = useApps("cpu");
  if (!snapshot) return <Missing />;
  const totalBusy = snapshot.cpu.userPercent + snapshot.cpu.privilegedPercent;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.cpu} title={t("cpu.title")}>
          <div className="hero-figure">
            <Tween className="huge num" value={snapshot.cpu.totalPercent} format={(n) => formatPercent(n, units.locale)} />
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
  const [range, setRange] = useTabState("memory", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("memory", "metric", "memory");
  const apps = useApps("memory");
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
            <Tween className="huge num" value={m.inUseBytes} format={(n) => formatBytes(n, units)} />
            <span className="faint">{t("label.ofTotal", { total: formatBytes(m.totalBytes, units) })}</span>
          </div>
          <Meter ratio={m.inUseBytes / Math.max(1, m.totalBytes)} color={TONE.memory} />
          <p className="hint faint">{t("memory.pressureHelp")}</p>
        </Card>
        <Card title={t("label.pressure")} tone={TONE.memory}>
          <div className="hero-figure">
            <Tween className="huge num" style={{ fontSize: 30 }} value={m.pressurePercent} format={(n) => formatPercent(n, units.locale)} />
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
  const [range, setRange] = useTabState("disk", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("disk", "metric", "disk");
  const apps = useApps("disk");
  if (!snapshot) return <Missing />;
  const d = snapshot.disk;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.disk} title={t("disk.title")}>
          <div className="rate-pair">
            <div>
              <span className="rate-label">{t("label.read")}</span>
              <Tween
                className="huge num"
                value={d.readBytesPerSecond}
                ease={false}
                format={(n) => formatRate(n, units, "storage")}
              />
              <Chart values={d.historyRead} color={TONE.disk} height={54} />
            </div>
            <div>
              <span className="rate-label">{t("label.write")}</span>
              <Tween
                className="huge num"
                value={d.writeBytesPerSecond}
                ease={false}
                format={(n) => formatRate(n, units, "storage")}
              />
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
  const [range, setRange] = useTabState("network", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("network", "metric", "connections");
  const apps = useApps("connections");
  if (!snapshot) return <Missing />;
  const n = snapshot.network;

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.network} title={t("network.title")}>
          <div className="rate-pair">
            <div>
              <span className="rate-label">{t("label.download")}</span>
              {/* A rate is shown as measured, not eased: the value between two one-second
                  samples is a figure that never happened, so easing it manufactures motion
                  out of noise. Pinning the unit to the window peak was tried and rejected —
                  it left a 10 KB/s upload reading as "0.01 MB/s" for as long as an old spike
                  stayed in the window, which trades a twitch for a loss of resolution. */}
              <Tween
                className="huge num"
                value={n.downloadBytesPerSecond}
                ease={false}
                format={(v) => formatRate(v, units, "network")}
              />
              <Chart values={n.historyDownload} color={TONE.network} height={54} />
            </div>
            <div>
              <span className="rate-label">{t("label.upload")}</span>
              <Tween
                className="huge num"
                value={n.uploadBytesPerSecond}
                ease={false}
                format={(v) => formatRate(v, units, "network")}
              />
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
              .map((a) => {
                // One row per card: state, how fast the link is, and both directions. The
                // shared break-row grid is three columns built for a meter, and dropping a
                // status badge into that meter column stretched it into what looked like an
                // empty progress bar.
                return (
                  <div key={a.name} className="adapter-row">
                    <span className="break-label" title={a.name}>
                      {a.name}
                    </span>
                    <Pill className="justify-self-start" tone={a.isUp ? "" : "faint"}>{a.isUp ? t("network.linkUp") : t("network.linkDown")}</Pill>
                    <span className="adapter-speed num faint">
                      {a.linkSpeedBitsPerSecond ? formatLinkSpeed(a.linkSpeedBitsPerSecond, units.locale) : "—"}
                    </span>
                    <span className="adapter-rate num">
                      ↓ {formatRate(a.downloadBytesPerSecond, units, "network")}
                    </span>
                    <span className="adapter-rate num">
                      ↑ {formatRate(a.uploadBytesPerSecond, units, "network")}
                    </span>
                  </div>
                );
              })}
            {n.adapters.filter((a) => !a.isLoopback).length === 0 && <p className="empty">{t("state.noData")}</p>}
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
  const [range, setRange] = useTabState("gpu", "range", 3600);
  if (!snapshot) return <Missing />;
  const g = snapshot.gpu;
  const appFor = (pid: number) => snapshot.apps.find((a) => a.pids.includes(pid));

  return (
    <div className="stack">
      <div className="hero-row">
        <Card className="hero" tone={TONE.gpu} title={t("gpu.title")}>
          <div className="hero-figure">
            <Tween className="huge num" value={g.totalPercent} format={(n) => formatPercent(n, units.locale)} />
            <span className="faint">{t("label.adapters", { count: g.adapters.length })}</span>
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
              {adapter.temperatureC === null && adapter.fanPercent === null && adapter.powerWatts === null ? (
                // Silence reads as a missing feature; this says which numbers the hardware
                // does not report and why, rather than showing nothing at all.
                <p className="hint faint">{t("gpu.noSensors")}</p>
              ) : (
                <div className="legend">
                  {adapter.temperatureC !== null && (
                    <span>{t("metric.temperature")} {formatTemp(adapter.temperatureC, units)}</span>
                  )}
                  {adapter.fanPercent !== null && <span>{t("label.fan")} {Math.round(adapter.fanPercent)}%</span>}
                  {adapter.powerWatts !== null && <span>{t("label.powerDraw")} {adapter.powerWatts.toFixed(1)} W</span>}
                </div>
              )}
            </div>
          ))}
        </Card>
      </div>

      {sectionShown(shown, "apps") && (
        <Card title={t("overview.topApps")}>
          <div className="rows">
            {g.perProcess.slice(0, 20).map((row) => {
              const owner = appFor(row.pid);
              const name = (owner && appLabel(owner, t).trim()) || `pid ${row.pid}`;
              return (
                <div key={row.pid} className="row">
                  <AppIcon name={name} seed={String(row.pid)} exePath={owner?.exePath || undefined} />
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
  const [range, setRange] = useTabState("battery", "range", 3600);
  const shown = visibleSections(settings, "battery");
  if (!snapshot) return <Missing />;
  const b = snapshot.battery;
  const temp = snapshot.temperature;

  return (
    <div className="stack">
      {/* On a machine with no battery there is nothing to report here, and the empty card
          would push the temperature panel the user can actually read below the fold. */}
      {b.hasBattery && (
        <div className="hero-row">
          <Card className="hero" tone={TONE.battery} title={t("battery.title")}>
            <div className="hero-figure">
              <Tween className="huge num" value={b.chargePercent} format={(n) => formatPercent(n, units.locale)} />
              <span className="faint">{b.isCharging ? t("label.charging") : b.onAcPower ? t("label.onAc") : t("label.timeRemaining")}</span>
            </div>
            <Meter ratio={(b.chargePercent ?? 0) / 100} color={b.isCritical ? "var(--m-danger)" : TONE.battery} />
            <div className="legend">
              <span>{t("label.timeRemaining")} {b.remainingSeconds ? formatDuration(b.remainingSeconds, units.locale) : "—"}</span>
              {b.powerWatts !== null && <span>{t("label.powerDraw")} {b.powerWatts?.toFixed(1)} W</span>}
            </div>
          </Card>
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
      )}

      {sectionShown(shown, "power") && (
        <Card title={t("temperature.title")} tone={TONE.temperature}>
          <div className="hero-figure">
            <Tween className="huge num" style={{ color: TONE.temperature }} value={temp.hottest} format={(n) => formatTemp(n, units)} />
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
          <AvailabilityNote state={temp.availability} t={t} elevate />
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

/**
 * "4 minutes ago" rather than a timestamp. The list is read to notice what just happened, and
 * working out how stale a line is from an absolute date is arithmetic nobody wants to do while
 * scanning; the exact time is still there on hover.
 */
function ago(atMs: number, t: Translate): string {
  const seconds = Math.max(0, (Date.now() - atMs) / 1000);
  if (seconds < 60) return t("time.justNow");
  if (seconds < 3600) return t("time.minutesAgo", { count: Math.floor(seconds / 60) });
  if (seconds < 86400) return t("time.hoursAgo", { count: Math.floor(seconds / 3600) });
  return t("time.daysAgo", { count: Math.floor(seconds / 86400) });
}

function InsightRows({ insights }: { insights: Insight[] }) {
  const { t, units, settings } = useStore();
  const a = settings.alerts;
  // Each line states the measurement and the limit it crossed, because "CPU above 388%" read
  // as though 388% were the threshold. The heat rule has no user-set limit, and it names the
  // busiest application rather than the one heating the machine, so that one says so.
  const detail = (insight: Insight) => {
    switch (insight.kind) {
      case "highCpu":
        return t("alert.highCpu", {
          value: formatPercent(insight.value, units.locale),
          limit: formatPercent(a.cpuPercent, units.locale),
        });
      case "growingMemory":
        return t("alert.growingMemory", {
          value: formatBytes(insight.value, units),
          limit: formatBytes(a.memoryGrowBytes, units),
        });
      case "heavyDisk":
        return t("alert.heavyDisk", {
          value: formatRate(insight.value, units, "storage"),
          limit: formatRate(a.diskBytesPerSecond, units, "storage"),
        });
      case "heavyNetwork":
        return t("alert.heavyNetwork", {
          value: formatRate(insight.value, units, "network"),
          limit: formatRate(a.networkBytesPerSecond, units, "network"),
        });
      case "heat":
        return t("alert.heat", { value: formatTemp(insight.value, units) });
    }
  };
  return (
    <div className="rows">
      {insights.map((insight) => (
        <div key={insight.id} className="row insight-row">
          <span className={`kind-dot ${insight.kind}`} />
          <div className="row-main">
            <span className="row-name">
              {insight.kind === "heat" ? (
                detail(insight)
              ) : (
                <>
                  <strong>{insight.appName}</strong> {detail(insight)}
                </>
              )}
            </span>
            <span className="row-sub faint" title={new Date(insight.atMs).toLocaleString(units.locale)}>
              {t("alert.for", {
                duration: formatDuration(insight.sustainedSeconds, units.locale, { short: true }),
              })}{" "}
              · {t("alert.lastSeen", { when: ago(insight.atMs, t) })}
              {insight.kind === "heat" ? ` · ${t("alert.busiest", { name: insight.appName })}` : ""}
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
