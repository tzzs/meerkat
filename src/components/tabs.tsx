import { memo, useCallback, useEffect, useMemo, useState, type CSSProperties } from "react";
import {
  formatBytes,
  formatDuration,
  formatLinkSpeed,
  formatPercent,
  formatRate,
  formatTemp,
  machineShareDisplay,
  type UnitPrefs,
} from "../lib/format";
import { appLabel, forLists } from "../lib/apps";
import { ipc, listenEvent } from "../lib/ipc";
import { formatHourLabel, iconPathForKey, rankingByHour, type RankingMetric } from "../lib/ranking";
import { buildSeries, formatAxisTime, formatHistoryValue, historyCeiling, historyUnit } from "../lib/series";
import { SPARK_CEILING } from "../lib/spark";
import { sectionShown, visibleSections } from "../lib/sections";
import { squarify } from "../lib/storage";
import { subscribe } from "../lib/subscribe";
import { useStore } from "../lib/store";
import type { AppInfo, DailyTotal, HistoryPoint, Insight, MetricKind, RankedApp, RankingRow, RunawayReason, RunawayVerdict, StorageMap, StorageMapProgress } from "../lib/types";
import { capacityTrend, sumDays } from "../lib/totals";
import { portMatches, portRows } from "../lib/ports";
import { useLoaded } from "../lib/use-loaded";
import type { Translate } from "../i18n";
import { Button } from "./ui/button";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import {
  AppIcon,
  AppTable,
  AvailabilityNote,
  Card,
  Chart,
  CoreGrid,
  HistoryPlot,
  Meter,
  Pill,
  Sections,
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

export const TONE: Record<MetricKind, string> = {
  cpu: "var(--m-cpu)",
  memory: "var(--m-memory)",
  disk: "var(--m-disk)",
  network: "var(--m-network)",
  gpu: "var(--m-gpu)",
  battery: "var(--m-battery)",
  temperature: "var(--m-temp)",
};

/**
 * Runs `load` now, then every `intervalMs` while the window can be seen, and once more the moment
 * it is shown again. These were bare intervals that kept calling into the backend for a window
 * nobody could see, and left a window that had been hidden for a minute stale until the tick
 * after it came back. `load` is told, through `alive`, whether its answer is still wanted.
 */
function usePolling(load: (alive: () => boolean) => void, intervalMs: number, deps: readonly unknown[]) {
  useEffect(() => {
    let wanted = true;
    const alive = () => wanted;
    const run = () => load(alive);
    run();
    const tick = () => {
      if (document.visibilityState !== "hidden") run();
    };
    const timer = window.setInterval(tick, intervalMs);
    document.addEventListener("visibilitychange", tick);
    return () => {
      wanted = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
    };
    // `load` is a closure over `deps`: the effect is rebuilt exactly when they change.
  }, deps);
}

/** One answer from history_series, with the moment it arrived: that is the window's right edge. */
interface LoadedSeries {
  /** The request it answers, so an answer for another metric or range is never shown for this one. */
  key: string;
  points: HistoryPoint[];
  bucketSeconds: number;
  atMs: number;
}

/**
 * The record of one metric over one range, refreshed every minute.
 *
 * The previous answer used to stay on screen until the next one arrived, so choosing "last 7
 * days" showed an hour of data under a seven-day label for as long as the call took. An answer
 * is kept with the request it belongs to and handed out only for that request; a refresh of the
 * same request keeps showing the old drawing until the new one replaces it.
 */
export function useSeries(metric: string, rangeSeconds: number): LoadedSeries | null {
  const key = `${metric}@${rangeSeconds}`;
  const [loaded, setLoaded] = useState<LoadedSeries | null>(null);
  usePolling(
    (alive) => {
      void ipc.historySeries(metric, rangeSeconds).then((series) => {
        if (alive()) setLoaded({ key, points: series.points, bucketSeconds: series.bucketSeconds, atMs: Date.now() });
      });
    },
    60_000,
    [metric, rangeSeconds],
  );
  return loaded?.key === key ? loaded : null;
}

/** The hourly record of the busiest apps: it gains one hour an hour, so it is asked for rarely. */
function useRanking(hours: number): RankingRow[] | null {
  const [rows, setRows] = useState<RankingRow[] | null>(null);
  usePolling(
    (alive) => {
      void ipc.historyRanking(hours).then((next) => {
        if (alive()) setRows(next);
      });
    },
    5 * 60_000,
    [hours],
  );
  return rows;
}

export function RangePicker({ value, onChange, tone }: { value: number; onChange: (seconds: number) => void; tone?: string }) {
  const { t } = useStore();
  return (
    <ToggleGroup
      type="single"
      value={String(value)}
      onValueChange={(next) => {
        if (next) onChange(Number(next));
      }}
      aria-label={t("label.range")}
      style={tone ? ({ ["--tone" as string]: tone } as CSSProperties) : undefined}
    >
      {RANGES.map((range) => (
        <ToggleChip key={range.seconds} value={String(range.seconds)}>
          {t(range.key)}
        </ToggleChip>
      ))}
    </ToggleGroup>
  );
}

/**
 * One metric's history: the means as a line, the highest value of each bucket as a lighter band
 * behind it, and the low, average and peak of the window in words.
 *
 * The window is [now - range, now] on a real time axis, so a machine that slept for three hours
 * has three blank hours and "last 7 days" is seven days wide. It takes `units` and `t` as props
 * rather than reading the store: the store changes every second, and a chart that subscribes
 * to it redraws every second for a picture that changes once a minute.
 */
const HistoryChart = memo(function HistoryChart({
  metric,
  range,
  tone,
  units,
  t,
  label,
  height = 110,
  scale = true,
}: {
  metric: string;
  range: number;
  tone: string;
  units: UnitPrefs;
  t: Translate;
  /** Names the series when a card holds more than one. */
  label?: string;
  height?: number;
  /** The time labels under the plot; a stack of charts on one axis needs them once. */
  scale?: boolean;
}) {
  const loaded = useSeries(metric, range);
  const built = useMemo(
    () => (loaded ? buildSeries(loaded.points, loaded.atMs - range * 1000, loaded.atMs, loaded.bucketSeconds) : null),
    [loaded, range],
  );
  // Still asking is not the same as nothing recorded, and neither is drawn as a flat line.
  if (!loaded || !built) {
    return (
      <div className="chart-placeholder" style={{ height }}>
        <span>…</span>
      </div>
    );
  }
  if (!built.stats) {
    return (
      <div className="chart-placeholder" style={{ height }}>
        <span>{t("state.noData")}</span>
      </div>
    );
  }
  const { min, avg, peak } = built.stats;
  const show = (value: number) => formatHistoryValue(metric, value, units);
  return (
    <div className="history">
      <div className="history-stats">
        {label && (
          <span className="history-label">
            <i className="swatch" style={{ background: tone }} />
            {label}
          </span>
        )}
        <span className="history-figures faint num">
          {t("history.low")} {show(min)} · {t("history.average")} {show(avg)} · {t("label.peak")} {show(peak)}
        </span>
      </div>
      <HistoryPlot
        built={built}
        ceiling={historyCeiling(historyUnit(metric), peak)}
        color={tone}
        height={height}
        label={label}
      />
      {scale && (
        <div className="history-scale faint">
          <span>{formatAxisTime(loaded.atMs - range * 1000, range, units.locale)}</span>
          <span>{t("history.now")}</span>
        </div>
      )}
    </div>
  );
});

/** The history card every tab ends with: the range picker and its series on one time axis. */
function HistoryCard({
  range,
  onRangeChange,
  series,
}: {
  range: number;
  onRangeChange: (seconds: number) => void;
  series: { metric: string; tone: string; label?: string }[];
}) {
  const { units, t } = useStore();
  const stacked = series.length > 1;
  return (
    <Card right={<RangePicker value={range} onChange={onRangeChange} tone={series[0]?.tone} />}>
      <div className="history-stack">
        {series.map((entry, index) => (
          <HistoryChart
            key={entry.metric}
            metric={entry.metric}
            range={range}
            tone={entry.tone}
            units={units}
            t={t}
            label={entry.label}
            height={stacked ? 72 : 110}
            scale={index === series.length - 1}
          />
        ))}
      </div>
    </Card>
  );
}

/**
 * The apps a list may show. Every caller used to pass `hideSystem = false`, which made
 * "Show Windows in lists" a switch connected to nothing; the setting now decides. The order is
 * the table's business (pins first, then its own sort), so none is imposed here.
 */
function useApps(): AppInfo[] {
  const { snapshot, settings } = useStore();
  return useMemo(
    () => forLists(snapshot?.apps ?? [], settings.showSystemAppsInLists),
    [snapshot, settings.showSystemAppsInLists],
  );
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

export function useTabState<T>(tab: string, field: string, initial: T): [T, (next: T) => void] {
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
  usePolling(
    (alive) => {
      void ipc.insights().then((next) => {
        if (alive()) setList(next);
      });
    },
    15_000,
    [],
  );
  return list;
}

/**
 * The watchdog's findings: fetched once, refreshed whenever the window is shown again (a hidden
 * window misses the `runaway` events the sampler sends), and updated live by those events.
 */
function useRunaways(): RunawayVerdict[] {
  const [list, setList] = useState<RunawayVerdict[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => void ipc.runaway().then((next) => alive && setList(next));
    load();
    const stop = subscribe(() =>
      listenEvent<RunawayVerdict[]>("runaway", (next) => {
        if (alive) setList(next);
      }),
    );
    const onShow = () => {
      if (document.visibilityState !== "hidden") load();
    };
    document.addEventListener("visibilitychange", onShow);
    return () => {
      alive = false;
      stop();
      document.removeEventListener("visibilitychange", onShow);
    };
  }, []);
  return list;
}

export function OverviewTab({ onOpenTab }: { onOpenTab: (tab: string) => void }) {
  const { snapshot, settings, units, t } = useStore();
  const order = visibleSections(settings, "overview");
  const [range, setRange] = useTabState("overview", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("overview", "metric", "memory");
  const apps = useApps();
  const insights = useInsights().filter((i) => !i.resolved);
  const runaways = useRunaways();
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
      // A battery that has not reported a charge is a dash, not a confident 0%.
      value:
        snapshot.battery.hasBattery && snapshot.battery.chargePercent !== null
          ? `${Math.round(snapshot.battery.chargePercent)}%`
          : "—",
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
            <Chart
              values={historyFor(snapshot, card.metric)}
              ceiling={SPARK_CEILING[card.metric]}
              color={TONE[card.metric]}
              variant="line"
              height={30}
            />
          </button>
        ))}
      </div>

      <Sections
        order={order}
        parts={{
          charts: (
            <Card title={t("overview.title")} right={<RangePicker value={range} onChange={setRange} />}>
              <div className="two-col">
                <div>
                  <h4 className="section-title">{t("metric.cpu")}</h4>
                  <HistoryChart metric="cpu" range={range} tone={TONE.cpu} units={units} t={t} />
                </div>
                <div>
                  <h4 className="section-title">{t("metric.memory")}</h4>
                  <HistoryChart metric="memory" range={range} tone={TONE.memory} units={units} t={t} />
                </div>
              </div>
            </Card>
          ),
          topApps: (
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
          ),
          worthALook: (
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
          ),
          runaway: <RunawayCard runaways={runaways} />,
          storage: (
            <Card title={t("overview.volumes")}>
              <VolumeList />
            </Card>
          ),
          byHour: <HourlyApps />,
        }}
      />
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
  const order = visibleSections(settings, "cpu");
  const [range, setRange] = useTabState("cpu", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("cpu", "metric", "cpu");
  const apps = useApps();
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
          <Chart values={snapshot.cpu.history} ceiling={100} color={TONE.cpu} height={80} />
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

      <Sections
        order={order}
        parts={{
          perCore: (
            <Card title={`${t("label.perCore")} · ${snapshot.machine.logicalCores}`}>
              <CoreGrid values={snapshot.cpu.corePercents} units={units} />
            </Card>
          ),
          history: (
            <HistoryCard range={range} onRangeChange={setRange} series={[{ metric: "cpu", tone: TONE.cpu }]} />
          ),
          apps: (
            <Card title={t("overview.topApps")}>
              <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
            </Card>
          ),
        }}
      />
    </div>
  );
}

export function MemoryTab({ search }: { search?: string }) {
  const { snapshot, settings, units, t } = useStore();
  const order = visibleSections(settings, "memory");
  const [range, setRange] = useTabState("memory", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("memory", "metric", "memory");
  const apps = useApps();
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
          <Chart values={m.history} ceiling={100} color={TONE.memory} height={72} />
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

      <Sections
        order={order}
        parts={{
          breakdown: (
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
          ),
          history: (
            <HistoryCard range={range} onRangeChange={setRange} series={[{ metric: "memory", tone: TONE.memory }]} />
          ),
          apps: (
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
          ),
        }}
      />
    </div>
  );
}

function useDailyTotals(days: number): DailyTotal[] {
  const [rows, setRows] = useState<DailyTotal[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => void ipc.dailyTotals(days).then((next) => alive && setRows(next));
    load();
    // The store moves once a minute; the live "today" figure fills the gap in between.
    const timer = window.setInterval(load, 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [days]);
  return rows;
}

/**
 * Today, the last 7 days and the last 30, from whole days Meerkat counted, with one bar per day.
 * The strings for this have been in the app since the start; the per-day store behind them is
 * what was missing, and without it a restart also used to zero "today".
 */
function TotalsCard({ kind }: { kind: "network" | "disk" }) {
  const { snapshot, units, t } = useStore();
  const rows = useDailyTotals(30);
  if (!snapshot) return null;
  const network = kind === "network";
  const live = network
    ? { download: snapshot.network.downloadBytesToday, upload: snapshot.network.uploadBytesToday }
    : { read: snapshot.disk.readBytesToday, write: snapshot.disk.writeBytesToday };
  const spans: [string, number][] = [
    [t("totals.today"), 1],
    [t("label.thisWeek"), 7],
    [t("label.thisMonth"), 30],
  ];
  const pick = (sums: ReturnType<typeof sumDays>) => (network ? [sums.download, sums.upload] : [sums.read, sums.write]);
  const labels = network ? [t("label.download"), t("label.upload")] : [t("label.read"), t("label.write")];
  const bars = rows.map((row, index) => {
    const isToday = index === rows.length - 1;
    const [a, b] = pick(sumDays([row], 1, isToday ? live : undefined));
    return a + b;
  });
  return (
    <Card title={t(network ? "totals.network" : "totals.disk")}>
      <div className="grid grid-cols-3 gap-4">
        {spans.map(([label, days]) => {
          const [first, second] = pick(sumDays(rows, days, live));
          return (
            <div key={days} className="min-w-0">
              <div className="stat-label">{label}</div>
              <div className="num truncate text-[15px] font-[650]">
                {network ? "↓ " : `${labels[0]} `}
                {formatBytes(first, units)}
              </div>
              <div className="num truncate text-[12px] text-ink-2">
                {network ? "↑ " : `${labels[1]} `}
                {formatBytes(second, units)}
              </div>
            </div>
          );
        })}
      </div>
      {bars.length > 1 && <Chart values={bars} color={TONE[kind]} variant="bars" height={48} />}
      {!network && <TopWritersToday />}
      <p className="hint faint">{t("totals.whileRunning")}</p>
    </Card>
  );
}

/**
 * Who wrote today's bytes. On an SSD writes are what wears the drive, and "the disk wrote 40 GB
 * today" only becomes something to act on once it says which programs did it.
 */
function TopWritersToday() {
  const { units, t, openDetail } = useStore();
  const writers = useLoaded(() => ipc.appRankingToday("write"), [], []);
  const shown = writers.filter((app) => app.writeBytes >= 1024 * 1024).slice(0, 5);
  if (shown.length === 0) return null;
  const peak = Math.max(1, ...shown.map((app) => app.writeBytes));
  return (
    <div className="mt-2">
      <h4 className="section-title">{t("totals.topWriters")}</h4>
      <div className="rows">
        {shown.map((app) => (
          <div
            key={app.key}
            className="row clickable"
            role="button"
            tabIndex={0}
            onClick={() => openDetail(app.key, { key: app.key, name: app.name, exePath: app.exePath, kind: app.kind })}
            onKeyDown={(event) =>
              event.key === "Enter" &&
              openDetail(app.key, { key: app.key, name: app.name, exePath: app.exePath, kind: app.kind })
            }
          >
            <AppIcon name={appLabel(app, t)} seed={app.key} exePath={app.exePath || undefined} />
            <div className="row-main">
              <span className="row-name">{appLabel(app, t)}</span>
              <span className="row-sub">{t(`kind.${app.kind}`)}</span>
            </div>
            <div className="row-meter">
              <Meter ratio={app.writeBytes / peak} color={TONE.disk} />
            </div>
            <span className="row-value num">{formatBytes(app.writeBytes, units)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

export function DiskTab() {
  const { snapshot, settings, units, t } = useStore();
  const order = visibleSections(settings, "disk");
  const [range, setRange] = useTabState("disk", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("disk", "metric", "disk");
  const apps = useApps();
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

      {sectionShown(order, "today") && <TotalsCard kind="disk" />}

      <Sections
        order={order}
        parts={{
          volumes: (
            <Card title={t("overview.volumes")}>
              <VolumeList />
            </Card>
          ),
          storageMap: (
            <Card title={t("section.disk.storageMap")}>
              <StorageMapCard />
            </Card>
          ),
          history: (
            <HistoryCard
              range={range}
              onRangeChange={setRange}
              series={[
                { metric: "diskRead", tone: TONE.disk, label: t("label.read") },
                { metric: "diskWrite", tone: TONE.disk, label: t("label.write") },
              ]}
            />
          ),
          apps: (
            <Card title={t("overview.topApps")}>
              <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
            </Card>
          ),
        }}
      />
    </div>
  );
}

export function NetworkTab() {
  const { snapshot, settings, units, t } = useStore();
  const order = visibleSections(settings, "network");
  const [range, setRange] = useTabState("network", "range", 3600);
  const [metric, setMetric] = useTabState<AppMetric>("network", "metric", "connections");
  const apps = useApps();
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

      {sectionShown(order, "totals") && <TotalsCard kind="network" />}

      <Sections
        order={order}
        parts={{
          adapters: (
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
          ),
          history: (
            <HistoryCard
              range={range}
              onRangeChange={setRange}
              series={[
                { metric: "download", tone: TONE.network, label: t("label.download") },
                { metric: "upload", tone: TONE.network, label: t("label.upload") },
              ]}
            />
          ),
          ports: <PortsCard />,
          connections: (
            <Card title={t("overview.topApps")} right={<p className="hint faint">{t("network.perAppHelp")}</p>}>
              <AppTable apps={apps} metric={metric} onMetricChange={setMetric} units={units} t={t} />
            </Card>
          ),
        }}
      />
    </div>
  );
}

/**
 * Who is listening, on which port, reachable from where. The question a developer asks of
 * netstat several times a day ("what is holding 3000?") answered by name, with the project the
 * process is serving and the way to stop it.
 */
function PortsCard() {
  const { snapshot, t, openDetail, requestQuit } = useStore();
  const [query, setQuery] = useState("");
  const [mine, setMine] = useTabState("network", "portsMine", true);
  if (!snapshot) return null;
  const rows = portRows(snapshot.network.listening ?? [], snapshot.apps)
    .filter((row) => !mine || row.app?.canQuit)
    .filter((row) => portMatches(row, query));
  const scope = (row: (typeof rows)[number]) =>
    row.scope === "network" ? t("network.scopeAll") : row.scope === "local" ? t("network.scopeLocal") : row.address;
  return (
    <Card
      title={t("network.ports")}
      right={
        <div className="flex items-center gap-2">
          <input
            className="find"
            placeholder={t("network.portsFilter")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            spellCheck={false}
          />
          <ToggleGroup type="single" value={mine ? "mine" : "all"} onValueChange={(next) => next && setMine(next === "mine")}>
            <ToggleChip value="mine">{t("network.portsMine")}</ToggleChip>
            <ToggleChip value="all">{t("history.allEvents")}</ToggleChip>
          </ToggleGroup>
        </div>
      }
    >
      <p className="hint faint">{t("network.portsHelp")}</p>
      {rows.length === 0 ? (
        <p className="empty faint">{t("network.portsEmpty")}</p>
      ) : (
        <div className="rows">
          {rows.map((row) => {
            const label = row.app ? appLabel(row.app, t) : `pid ${row.pid}`;
            return (
              <div
                key={`${row.port}:${row.pid}`}
                className="port-row clickable"
                role="button"
                tabIndex={0}
                onClick={() => row.app && openDetail(row.app.key)}
                onKeyDown={(event) => event.key === "Enter" && row.app && openDetail(row.app.key)}
              >
                <span className="port-number num">{row.port}</span>
                <span className={`port-scope ${row.scope === "network" ? "open" : ""}`}>{scope(row)}</span>
                <AppIcon name={label} seed={row.app?.key ?? String(row.pid)} exePath={row.app?.exePath || undefined} />
                <span className="port-app">
                  <span className="row-name">{label}</span>
                  {row.app?.project && <span className="row-sub">{row.app.project}</span>}
                </span>
                {row.app?.canQuit ? (
                  <button
                    className="row-quit"
                    aria-label={t("action.quitApp")}
                    onClick={(event) => {
                      event.stopPropagation();
                      if (row.app) requestQuit(row.app);
                    }}
                  >
                    ×
                  </button>
                ) : (
                  <span />
                )}
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}

export function GpuTab() {
  const { snapshot, settings, units, t } = useStore();
  const order = visibleSections(settings, "gpu");
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
          <Chart values={g.history} ceiling={100} color={TONE.gpu} height={80} />
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
                  {adapter.fanPercent !== null && (
                    <span>
                      {adapter.fanPercent > 0
                        ? `${t("label.fan")} ${Math.round(adapter.fanPercent)}%`
                        : t("label.fanStopped")}
                    </span>
                  )}
                  {adapter.powerWatts !== null && <span>{t("label.powerDraw")} {adapter.powerWatts.toFixed(1)} W</span>}
                </div>
              )}
            </div>
          ))}
        </Card>
      </div>

      <Sections
        order={order}
        parts={{
          apps: (
            <Card title={t("overview.topApps")}>
              <div className="rows">
                {g.perProcess.slice(0, 20).map((row) => {
                  const owner = appFor(row.pid);
                  const name = owner?.name?.trim() || t("label.pidName", { pid: row.pid });
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
          ),
          history: (
            <HistoryCard range={range} onRangeChange={setRange} series={[{ metric: "gpu", tone: TONE.gpu }]} />
          ),
        }}
      />
    </div>
  );
}

export function BatteryTab() {
  const { snapshot, settings, units, t } = useStore();
  const [range, setRange] = useTabState("battery", "range", 3600);
  const order = visibleSections(settings, "battery");
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
            <CapacityTrend />
            <AvailabilityNote state={b.source} t={t} />
          </Card>
        </div>
      )}

      <Sections
        order={order}
        parts={{
          power: (
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
              <AvailabilityNote state={temp.availability} t={t} />
            </Card>
          ),
          history: (
            <HistoryCard
              range={range}
              onRangeChange={setRange}
              series={
                b.hasBattery
                  ? [{ metric: "battery", tone: TONE.battery }]
                  : [{ metric: "temp", tone: TONE.temperature }]
              }
            />
          ),
        }}
      />
    </div>
  );
}

/** The weekly full-charge readings Windows keeps, as a line: how fast this battery is wearing. */
function CapacityTrend() {
  const { snapshot, units, t } = useStore();
  const points = snapshot?.battery.capacityHistory ?? [];
  const trend = capacityTrend(points);
  if (!trend) return null;
  return (
    <div className="stat">
      <span className="stat-label">{t("battery.capacityTrend")}</span>
      <Chart
        values={points.filter((p) => p.designWh > 0).map((p) => (p.fullChargeWh / p.designWh) * 100)}
        ceiling={100}
        color={TONE.battery}
        variant="line"
        height={40}
      />
      <span className="stat-sub">
        {t("battery.capacityTrendRange", {
          from: formatPercent(trend.from, units.locale),
          to: formatPercent(trend.to, units.locale),
          weeks: trend.weeks,
        })}
      </span>
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
              {[
                volume.name,
                volume.fileSystem,
                volume.isSolidState ? t("label.ssd") : null,
                volume.isRemovable ? t("label.removable") : null,
              ]
                .filter(Boolean)
                .join(" · ")}
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
 * A treemap of what is taking the space on one volume, laid out by `squarify` and drawn as plain
 * absolutely-positioned divs. The walk is the expensive part and is only started by the button;
 * the boxes themselves never move on their own, so nothing here runs on the sampler's tick.
 */
function StorageMapCard() {
  const { snapshot, units, t } = useStore();
  const volumes = snapshot?.disk.volumes ?? [];
  const [volumeId, setVolumeId] = useTabState<string>("disk", "storageMapVolume", volumes[0]?.id ?? "");
  const [result, setResult] = useState<StorageMap | null>(null);
  const [scanning, setScanning] = useState(false);
  const [failed, setFailed] = useState(false);
  const [progress, setProgress] = useState<StorageMapProgress | null>(null);

  // A drive can be unplugged between visits; fall back to the first volume that is still there.
  const activeId = volumes.some((v) => v.id === volumeId) ? volumeId : volumes[0]?.id ?? "";

  useEffect(() => {
    // Progress arrives by event; only a walk of the volume being mapped counts. `subscribe`
    // because `listen` resolves its unlisten after a round trip, which outlives the effect.
    return subscribe(() =>
      listenEvent<StorageMapProgress>("storage-map-progress", (event) => {
        setProgress((current) => (event.volumeId === activeId ? event : current));
      }),
    );
  }, [activeId]);

  // A different volume is a different answer, not the old one shown under a new label.
  useEffect(() => {
    setResult(null);
    setFailed(false);
    setProgress(null);
    setScanning(false);
  }, [activeId]);

  const rects = useMemo(() => (result ? squarify(result.boxes.map((box) => box.bytes)) : []), [result]);

  if (volumes.length === 0) return <p className="empty">{t("storageMap.empty")}</p>;

  const scan = () => {
    setScanning(true);
    setFailed(false);
    setResult(null);
    setProgress(null);
    void ipc.storageMapScan(activeId).then((outcome) => {
      setScanning(false);
      if (outcome.ok && outcome.value) setResult(outcome.value);
      else setFailed(true);
    });
  };

  const cancel = () => {
    void ipc.storageMapCancel();
  };

  return (
    <div className="storage-map-card">
      {volumes.length > 1 && (
        <ToggleGroup
          type="single"
          value={activeId}
          onValueChange={(next) => {
            if (next) setVolumeId(next);
          }}
          aria-label={t("section.disk.storageMap")}
        >
          {volumes.map((v) => (
            <ToggleChip key={v.id} value={v.id}>
              {v.mountPoint}
            </ToggleChip>
          ))}
        </ToggleGroup>
      )}

      {scanning ? (
        <div className="storage-map-progress">
          <span className="hint faint">
            {t("storageMap.scanning", {
              files: progress?.files ?? 0,
              bytes: formatBytes(progress?.bytes ?? 0, units),
            })}
          </span>
          <Button variant="chip" onClick={cancel}>
            {t("action.cancel")}
          </Button>
        </div>
      ) : failed ? (
        <>
          <p className="empty">{t("storageMap.failed")}</p>
          <Button variant="chip" onClick={scan}>
            {t("storageMap.scan")}
          </Button>
        </>
      ) : result?.cancelled ? (
        <>
          <p className="hint faint">{t("storageMap.cancelled")}</p>
          <Button variant="chip" onClick={scan}>
            {t("storageMap.scan")}
          </Button>
        </>
      ) : result ? (
        <>
          <div className="storage-map">
            {result.boxes.map((box, i) => {
              const rect = rects[i];
              if (!rect || rect.w <= 0 || rect.h <= 0) return null;
              // A nameless box that is neither free nor unreadable is the measured space that
              // was not one of the biggest folders, folded into one region so the map still
              // covers the whole volume.
              const label = box.free
                ? t("label.free")
                : box.unknown
                  ? t("storageMap.other")
                  : box.name || box.path || t("storageMap.rest");
              const kind = box.free ? "free" : box.unknown ? "unknown" : box.name || box.path ? "named" : "rest";
              return (
                <div
                  key={`${i}:${box.path}:${kind}`}
                  className={`map-box ${kind}`}
                  style={{
                    left: `${rect.x * 100}%`,
                    top: `${rect.y * 100}%`,
                    width: `${rect.w * 100}%`,
                    height: `${rect.h * 100}%`,
                  }}
                  title={`${label} · ${formatBytes(box.bytes, units)}`}
                >
                  {rect.w > 0.14 && rect.h > 0.14 && (
                    <>
                      <span className="map-box-name">{label}</span>
                      <span className="map-box-size num">{formatBytes(box.bytes, units)}</span>
                    </>
                  )}
                </div>
              );
            })}
          </div>
          <p className="hint faint">
            {t("storageMap.reconciled", {
              measured: formatBytes(result.measuredBytes, units),
              used: formatBytes(result.usedBytes, units),
              unreadable: formatBytes(Math.max(0, result.usedBytes - result.measuredBytes), units),
            })}
          </p>
          {result.unreadable > 0 && (
            <p className="hint faint">{t("storageMap.unreadableFolders", { count: result.unreadable })}</p>
          )}
          <Button variant="chip" onClick={scan}>
            {t("storageMap.scan")}
          </Button>
        </>
      ) : (
        <>
          <p className="hint faint">{t("storageMap.help")}</p>
          <Button variant="chip" onClick={scan}>
            {t("storageMap.scan")}
          </Button>
        </>
      )}
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

/** One short sentence for why a process was flagged, from the reading the verdict carries. */
function runawayReason(reason: RunawayReason, t: Translate, units: UnitPrefs): string {
  switch (reason.reason) {
    case "cpuSustained":
      return t("runaway.cpuSustained", {
        duration: formatDuration(reason.sustainedSeconds, units.locale, { short: true }),
      });
    case "projectGone":
      return t("runaway.projectGone");
    case "memoryCeiling":
      return t("runaway.memoryCeiling", { value: formatBytes(reason.memoryBytes, units) });
  }
}

/**
 * The runaway list: one row per dev runtime the watchdog has called stuck, naming it and why.
 * A click opens its detail, where the existing quit path takes over — the watchdog itself never
 * ends anything, and a dismiss keeps the app quiet for the rest of the day.
 */
function RunawayCard({ runaways }: { runaways: RunawayVerdict[] }) {
  const { snapshot, t, units, openDetail } = useStore();
  return (
    <Card title={t("overview.runaway")} right={runaways.length > 0 ? <Pill tone="danger">{runaways.length}</Pill> : undefined}>
      <p className="hint faint">{t("overview.runawayHelp")}</p>
      {runaways.length === 0 ? (
        <p className="empty">{t("overview.runawayEmpty")}</p>
      ) : (
        <div className="rows">
          {runaways.map((verdict) => {
            const live = snapshot?.apps.find((a) => a.key === verdict.appKey) ?? null;
            const label = live?.name.trim() || verdict.appName.trim() || verdict.appKey;
            const seed = live ? { key: live.key, name: live.name, exePath: live.exePath, kind: live.kind } : undefined;
            return (
              <div
                key={verdict.id}
                className="row clickable"
                role="button"
                tabIndex={0}
                onClick={() => openDetail(verdict.appKey, seed)}
                onKeyDown={(event) => event.key === "Enter" && openDetail(verdict.appKey, seed)}
              >
                <AppIcon name={label} seed={verdict.appKey} exePath={live?.exePath || undefined} />
                <div className="row-main">
                  <span className="row-name">{label}</span>
                  <span className="row-sub">{runawayReason(verdict.reason, t, units)}</span>
                </div>
                <button
                  className="row-quit"
                  aria-label={t("action.dismiss")}
                  onClick={(event) => {
                    event.stopPropagation();
                    ipc.dismissRunaway(verdict.id);
                  }}
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}

function InsightRows({ insights }: { insights: Insight[] }) {
  const { t, units, settings } = useStore();
  const a = settings.alerts;
  // A figure that is not a number is a dash: an old record can carry one, and "0%" would be a
  // reading nobody took.
  const share = (percent: number) => {
    const shown = machineShareDisplay(percent, units);
    return shown === null ? "—" : formatPercent(shown, units.locale);
  };
  // Each line states the measurement and the limit it crossed, because "CPU above 388%" read
  // as though 388% were the threshold. The heat rule has no user-set limit, and it names the
  // busiest application rather than the one heating the machine, so that one says so. Memory
  // growth is a change over a stretch of time and says which stretch; it has no sustained
  // "hold" to report, and nothing here claims it is still going.
  const detail = (insight: Insight) => {
    switch (insight.kind) {
      case "highCpu":
        // Measured against the whole machine, shown in the unit the rest of the window uses.
        return t("alert.highCpu", { value: share(insight.value), limit: share(a.cpuPercent) });
      case "growingMemory":
        return t("alert.growingMemory", {
          value: Number.isFinite(insight.value) ? formatBytes(Math.max(0, insight.value), units) : "—",
          duration: formatDuration(insight.sustainedSeconds, units.locale, { short: true }),
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
  const subline = (insight: Insight) =>
    [
      insight.kind === "growingMemory"
        ? null
        : t("alert.for", { duration: formatDuration(insight.sustainedSeconds, units.locale, { short: true }) }),
      t("alert.lastSeen", { when: ago(insight.atMs, t) }),
      insight.kind === "heat" ? t("alert.busiest", { name: insight.appName }) : null,
    ]
      .filter(Boolean)
      .join(" · ");
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
              {subline(insight)}
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

/**
 * The spec's "busiest apps by hour": for each of the last 48 hours, the three apps that used
 * the most, by memory or by CPU. It comes from the hourly record rather than from anything live,
 * so it answers "what was eating the machine at 3am", which the table above cannot.
 *
 * Twelve hours are shown and the rest are one click away: forty-eight rows of three apps would
 * push everything below this card off the screen.
 */
const HOURS_SHOWN = 12;

function HourlyApps() {
  const { units, t } = useStore();
  const rows = useRanking(48);
  const [metric, setMetric] = useTabState<RankingMetric>("overview", "hourMetric", "memory");
  const [expanded, setExpanded] = useState(false);
  const hours = useMemo(() => rankingByHour(rows ?? [], metric), [rows, metric]);
  const shown = expanded ? hours : hours.slice(0, HOURS_SHOWN);
  const now = Date.now();

  const value = (app: RankedApp) => {
    if (metric === "memory") return formatBytes(app.value, units);
    const percent = machineShareDisplay(app.value, units);
    return percent === null ? "—" : formatPercent(percent, units.locale, percent < 10 ? 1 : 0);
  };

  return (
    <Card
      title={t("overview.byHour")}
      right={
        <ToggleGroup
          type="single"
          value={metric}
          onValueChange={(next) => {
            if (next) setMetric(next as RankingMetric);
          }}
          aria-label={t("overview.byHourMetric")}
        >
          <ToggleChip value="memory">{t("metric.memory")}</ToggleChip>
          <ToggleChip value="cpu">{t("metric.cpu")}</ToggleChip>
        </ToggleGroup>
      }
    >
      <p className="hint faint">{t("overview.byHourHelp")}</p>
      {rows === null ? (
        <p className="empty">…</p>
      ) : hours.length === 0 ? (
        <p className="empty">{t("overview.byHourEmpty")}</p>
      ) : (
        <>
          <div className="hour-list">
            {shown.map((hour) => (
              <div key={hour.hourStartMs} className="hour-row">
                <span className="hour-label num faint">{formatHourLabel(hour.hourStartMs, now, units.locale)}</span>
                <div className="hour-apps">
                  {hour.apps.map((app) => {
                    const label = appLabel(app, t);
                    return (
                      <span key={app.key} className="hour-app" title={label}>
                        <AppIcon name={label} seed={app.key} exePath={iconPathForKey(app.key)} />
                        <span className="hour-app-name">{label}</span>
                        <span className="hour-app-value num faint">{value(app)}</span>
                      </span>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
          {hours.length > HOURS_SHOWN && (
            <Button variant="chip" className="hour-more" onClick={() => setExpanded(!expanded)}>
              {expanded ? t("overview.byHourLess") : t("overview.byHourMore", { count: hours.length })}
            </Button>
          )}
        </>
      )}
    </Card>
  );
}

function Missing() {
  const { t } = useStore();
  return <p className="empty">{t("state.noData")}</p>;
}
