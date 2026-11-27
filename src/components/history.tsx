import { useEffect, useMemo, useRef, useState } from "react";
import { useLoaded } from "../lib/use-loaded";
import { cpuDisplay, formatBytes, formatDuration, formatPercent, formatRate } from "../lib/format";
import { ipc } from "../lib/ipc";
import { useStore, type AppSeed } from "../lib/store";
import { sectionShown, visibleSections } from "../lib/sections";
import { appLabel } from "../lib/apps";
import { eventText, isProblem } from "../lib/events";
import { buildSeries, seriesPaths } from "../lib/series";
import { blankSpans, recordedSpan, snapToBucket } from "../lib/timeline";
import type { AppUsage, HistoryPoint, RankBy, SystemEvent } from "../lib/types";
import type { Translate } from "../i18n";
import type { Key } from "../i18n/en";
import { AppIcon, Card, Meter } from "./ui";
import { Button } from "./ui/button";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import { RangePicker, TONE, useSeries, useTabState } from "./tabs";

/** What the timeline draws, and what the ranking under it sorts by, for each choice. */
const MEASURES: { by: RankBy; series: string; label: Key; tone: string; percent: boolean }[] = [
  { by: "cpu", series: "cpu", label: "metric.cpu", tone: TONE.cpu, percent: true },
  { by: "memory", series: "memory", label: "metric.memory", tone: TONE.memory, percent: true },
  { by: "write", series: "diskWrite", label: "history.diskWrites", tone: TONE.disk, percent: false },
  { by: "gpu", series: "gpu", label: "metric.gpu", tone: TONE.gpu, percent: true },
];

function useSystemEvents(rangeSeconds: number): SystemEvent[] {
  const [events, setEvents] = useState<SystemEvent[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => void ipc.systemEvents(rangeSeconds).then((next) => alive && setEvents(next));
    load();
    const timer = window.setInterval(load, 120_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [rangeSeconds]);
  return events;
}

/** A moment, as precisely as the range it was picked from needs. */
function formatWhen(atMs: number, rangeSeconds: number, locale: string): string {
  const options: Intl.DateTimeFormatOptions =
    // A day's range starts and ends at the same clock time, so it needs the date to tell them apart.
    rangeSeconds < 86_400
      ? { hour: "2-digit", minute: "2-digit" }
      : { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" };
  return new Intl.DateTimeFormat(locale, options).format(atMs);
}

function seedOf(app: AppUsage): AppSeed {
  return { key: app.key, name: app.name, exePath: app.exePath, kind: app.kind };
}

export function HistoryTab() {
  const { snapshot, settings, t, units } = useStore();
  const shown = visibleSections(settings, "history");
  const [range, setRange] = useTabState("history", "range", 86_400);
  const [by, setBy] = useTabState<RankBy>("history", "by", "cpu");
  const [moment, setMoment] = useTabState<number | null>("history", "moment", null);
  const [problemsOnly, setProblemsOnly] = useTabState("history", "problemsOnly", true);
  const measure = MEASURES.find((m) => m.by === by) ?? MEASURES[0];
  const series = useSeries(measure.series, range);
  const events = useSystemEvents(range);
  const now = snapshot?.takenAtMs ?? Date.now();
  const top = useRef<HTMLDivElement>(null);

  /**
   * Picking a moment from the event list. It lands on the same bucket grid a click on the chart
   * lands on, so the marker is drawn at a place that has a reading (and a value in its pill)
   * rather than at the exact second Windows logged, which sits between two buckets.
   */
  const pick = (at: number) => {
    const from = now - range * 1000;
    const bucket = snapToBucket(at, series?.bucketSeconds ?? 60);
    setMoment(Math.min(now, Math.max(from, bucket)));
    top.current?.scrollIntoView({ block: "start", behavior: "smooth" });
  };

  return (
    <div className="stack" ref={top}>
      {sectionShown(shown, "timeline") && (
        <Card
          title={t("history.title")}
          right={
            <div className="flex flex-wrap items-center justify-end gap-2">
              <ToggleGroup
                type="single"
                value={by}
                onValueChange={(next) => next && setBy(next as RankBy)}
                aria-label={t("list.sortBy")}
              >
                {MEASURES.map((m) => (
                  <ToggleChip key={m.by} value={m.by}>
                    {t(m.label)}
                  </ToggleChip>
                ))}
              </ToggleGroup>
              <RangePicker value={range} onChange={setRange} />
            </div>
          }
        >
          <TimelineChart
            points={series?.points ?? []}
            bucketSeconds={series?.bucketSeconds ?? 60}
            from={now - range * 1000}
            to={now}
            tone={measure.tone}
            name={t(measure.label)}
            format={(value) =>
              measure.percent ? formatPercent(value, units.locale) : formatRate(value, units, "storage")
            }
            fixedMax={measure.percent ? 100 : undefined}
            events={events}
            selected={moment}
            onSelect={setMoment}
            label={(at) => formatWhen(at, range, units.locale)}
            loading={series === null}
            t={t}
            shortDuration={(ms) => formatDuration(ms / 1000, units.locale, { short: true })}
            viewKey={`${range}:${by}`}
          />
          <p className="hint faint">{t("history.clickHint")}</p>
          {moment !== null && <MomentPanel at={moment} range={range} events={events} onClear={() => setMoment(null)} />}
        </Card>
      )}

      {sectionShown(shown, "ranking") && <RankingCard range={range} by={by} />}

      {sectionShown(shown, "events") && (
        <Card
          title={t("history.events")}
          right={
            <ToggleGroup
              type="single"
              value={problemsOnly ? "problems" : "all"}
              onValueChange={(next) => next && setProblemsOnly(next === "problems")}
            >
              <ToggleChip value="problems">{t("history.onlyProblems")}</ToggleChip>
              <ToggleChip value="all">{t("history.allEvents")}</ToggleChip>
            </ToggleGroup>
          }
        >
          <EventList
            events={problemsOnly ? events.filter(isProblem) : events}
            range={range}
            onPick={pick}
          />
          <p className="hint faint">{t("history.eventsHelp")}</p>
        </Card>
      )}
    </div>
  );
}

/**
 * The machine-wide figure over the chosen range, with the system's events marked along the
 * bottom edge. It is a picker as much as a chart: a click chooses the moment the panel below
 * describes, and a second click on the same moment gives the choice back — a stuck selection had
 * no way out, and the vertical bar it drew said nothing about what it was. Gaps where Meerkat was
 * not running are drawn as hatched stretches named as such, so an empty chart reads as "nothing
 * was recorded here" rather than as a broken chart with one bar in it.
 */
function TimelineChart({
  points,
  bucketSeconds,
  from,
  to,
  tone,
  name,
  format,
  fixedMax,
  events,
  selected,
  onSelect,
  label,
  loading,
  t,
  shortDuration,
  viewKey,
}: {
  points: HistoryPoint[];
  bucketSeconds: number;
  from: number;
  to: number;
  tone: string;
  /** The measure's own name, for the legend and for the plot's accessible name. */
  name: string;
  format: (value: number) => string;
  fixedMax?: number;
  events: SystemEvent[];
  selected: number | null;
  /** Null clears the selection. */
  onSelect: (at: number | null) => void;
  label: (at: number) => string;
  /** Still asking for the series. "Nothing recorded" and "not answered yet" are not the same thing. */
  loading: boolean;
  t: Translate;
  /** A length of time in words, for "only the 26 minutes that were recorded". */
  shortDuration: (ms: number) => string;
  /** Changes when the range or metric does: a zoom into the old series means nothing on a new one. */
  viewKey: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(720);
  const [hover, setHover] = useState<number | null>(null);
  const [zoom, setZoom] = useState(false);
  useEffect(() => {
    const element = box.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(Math.max(200, entry.contentRect.width)));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  // A zoom into the previous series would put its labels over a different picture.
  useEffect(() => setZoom(false), [viewKey]);

  const height = 168;
  const band = 16;
  const top = 10;
  const plot = height - band - top;
  const span = Math.max(1, to - from);
  const visible = useMemo(() => points.filter((p) => p.atMs >= from && p.atMs <= to), [points, from, to]);
  const recorded = useMemo(
    () => recordedSpan(visible, from, to, bucketSeconds),
    [visible, from, to, bucketSeconds],
  );
  const blanks = useMemo(() => blankSpans(visible, from, to, bucketSeconds), [visible, from, to, bucketSeconds]);
  // A day's range with half an hour of record is a sliver against one edge; offering to draw only
  // what was recorded is what makes that half hour legible. Nothing to offer when the record
  // already fills most of the range.
  const recordedMs = recorded === null ? 0 : recorded.toMs - recorded.fromMs;
  const zoomable = recordedMs > 0 && recordedMs < 0.6 * span;
  const zoomed = zoom && zoomable && recorded !== null;
  // A live series can grow past the sparse threshold while the user is looking at it. Drop the
  // zoom then, rather than leaving a hidden toggle on and a chart that no longer says it is zoomed.
  useEffect(() => {
    if (!zoomable) setZoom(false);
  }, [zoomable]);
  const view = zoomed && recorded !== null ? recorded : { fromMs: from, toMs: to };
  const viewSpan = Math.max(1, view.toMs - view.fromMs);
  const x = (at: number) => ((at - view.fromMs) / viewSpan) * width;
  // Where each blank stretch falls on screen, clipped to the plot: zoomed in, a stretch can start
  // days to the left of the view, and an unclipped label would be drawn far outside the card.
  // Nothing is marked as missing until the series has actually answered.
  const blanksPx = loading
    ? []
    : blanks
        .map((blank) => {
          const left = Math.max(0, x(blank.fromMs));
          const right = Math.min(width, x(blank.toMs));
          return { left, right, width: Math.max(0, right - left) };
        })
        .filter((blank) => blank.width > 0);
  // The top of the scale, from the means the line draws: the peak a bucket reached is a detail,
  // and a band of it behind the line read as a second, noisier series.
  const max = fixedMax ?? Math.max(1, ...visible.map((p) => p.value)) * 1.15;
  const y = (value: number) => top + plot - (Math.min(value, max) / max) * plot;
  // A bucket's reading stands for its whole bucket, so the last one is held to the end of it.
  // Drawn only to its own timestamp, the line stopped one bucket short of the right edge, which
  // on a range zoomed to a quarter of an hour left the last two-fifths of the chart looking empty.
  const drawn = useMemo(() => {
    const last = visible[visible.length - 1];
    if (last === undefined) return visible;
    const tail = Math.min(view.toMs, last.atMs + bucketSeconds * 1000);
    return tail > last.atMs ? [...visible, { ...last, atMs: tail }] : visible;
  }, [visible, view.toMs, bucketSeconds]);
  const built = useMemo(
    () => buildSeries(drawn, view.fromMs, view.toMs, bucketSeconds),
    [drawn, view.fromMs, view.toMs, bucketSeconds],
  );
  const paths = useMemo(() => seriesPaths(built, width, plot, max), [built, width, plot, max]);

  const shownEvents = events.filter((event) => event.atMs >= view.fromMs && event.atMs <= view.toMs);
  const atOf = (clientX: number) => {
    const rect = box.current?.getBoundingClientRect();
    if (!rect) return view.fromMs;
    const fraction = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return view.fromMs + fraction * viewSpan;
  };
  const bucketMs = bucketSeconds * 1000;
  // The view's edges need not be bucket-aligned (the right edge is "now"), so a snapped Home or
  // a click at the very left edge is kept inside the window instead of selecting before it.
  const moment = (at: number) => Math.min(view.toMs, Math.max(view.fromMs, snapToBucket(at, bucketSeconds)));
  const pick = (at: number) => {
    const next = moment(at);
    // The same click always lands in the same bucket, including the bucket's latter half. An
    // adjacent bucket is a different choice even when a coarse range draws it only a few pixels
    // away, so a fixed pixel tolerance was wrong on the 30-day view.
    onSelect(selected !== null && next === selected ? null : next);
  };
  const nearest = (at: number) =>
    visible.reduce<HistoryPoint | null>(
      (best, p) => (best === null || Math.abs(p.atMs - at) < Math.abs(best.atMs - at) ? p : best),
      null,
    );
  const hovered = hover === null ? null : nearest(hover);
  const readout = hovered !== null && hover !== null && Math.abs(hovered.atMs - hover) < bucketMs * 1.5 ? hovered.value : null;
  const picked = selected === null ? null : (visible.find((p) => p.atMs === selected) ?? null);
  const selectedShown = selected !== null && selected >= view.fromMs && selected <= view.toMs;
  // The widest stretch with nothing recorded is named, so the blank part of a chart says what it
  // is rather than looking like a chart that failed to draw.
  const widest = blanksPx.reduce<{ left: number; right: number; width: number } | null>(
    (best, blank) => (best === null || blank.width > best.width ? blank : best),
    null,
  );
  const clamp = (at: number, edge: number) => Math.min(Math.max(x(at), edge), Math.max(edge, width - edge));

  const step = (delta: number) => {
    const base = selected ?? moment((view.fromMs + view.toMs) / 2);
    onSelect(Math.min(view.toMs, Math.max(view.fromMs, base + delta * bucketMs)));
  };

  return (
    <div className="timeline">
      <div className="timeline-head">
        <span className="timeline-key">
          <i className="swatch" style={{ background: tone }} />
          {name}
        </span>
        <span className="timeline-key">
          <i className="tick" />
          <i className="tick problem" />
          {t("history.events")}
        </span>
        <span className="timeline-grow" />
        {zoomable && (
          <Button
            variant="chip"
            className="timeline-zoom"
            aria-pressed={zoomed}
            onClick={() => setZoom((on) => !on)}
          >
            {zoomed ? t("history.showAll") : t("history.onlyRecorded", { value: shortDuration(recordedMs) })}
          </Button>
        )}
      </div>
      <span className="sr-only" aria-live="polite" aria-atomic="true">
        {selected === null
          ? t("history.selectionCleared")
          : `${label(selected)} · ${picked ? format(picked.value) : t("history.noRecord")}`}
      </span>
      <div
        ref={box}
        className="timeline-plot"
        role="group"
        tabIndex={0}
        aria-label={`${name}, ${t("history.clickHint")}`}
        onPointerMove={(event) => setHover(atOf(event.clientX))}
        onPointerLeave={() => setHover(null)}
        onClick={(event) => pick(atOf(event.clientX))}
        onKeyDown={(event) => {
          switch (event.key) {
            case "ArrowLeft":
              step(-1);
              break;
            case "ArrowRight":
              step(1);
              break;
            case "PageUp":
              step(-10);
              break;
            case "PageDown":
              step(10);
              break;
            case "Home":
              onSelect(moment(view.fromMs));
              break;
            case "End":
              onSelect(moment(view.toMs));
              break;
            case "Escape":
              onSelect(null);
              break;
            default:
              return;
          }
          event.preventDefault();
        }}
      >
        <svg width={width} height={height} role="img" aria-label={`${name}: ${label(view.fromMs)} – ${label(view.toMs)}`}>
          <defs>
            <pattern id="timeline-blank" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
              <line x1="0" y1="0" x2="0" y2="6" stroke="var(--text-3)" strokeWidth="1" opacity="0.28" />
            </pattern>
          </defs>
          {blanksPx.map((blank, index) => (
            <rect
              key={`blank-${index}`}
              x={blank.left}
              y={top}
              width={blank.width}
              height={plot}
              fill="url(#timeline-blank)"
              opacity={0.55}
            />
          ))}
          <line x1={0} x2={width} y1={top + plot / 2} y2={top + plot / 2} stroke="var(--line)" strokeDasharray="2 4" />
          {/* A stronger axis separates the metric's plot from the system-event ticks below it. */}
          <line x1={0} x2={width} y1={top + plot} y2={top + plot} stroke="var(--line-strong)" />
          <g transform={`translate(0,${top})`}>
            {paths.area && <path d={paths.area} fill={tone} opacity={0.16} />}
            {paths.line && (
              <path d={paths.line} fill="none" stroke={tone} strokeWidth={1.5} strokeLinejoin="round" />
            )}
          </g>
          {/* The scale, so a curve can be read as a number rather than only against itself. */}
          {visible.length > 0 && (
            <g className="timeline-scale">
              <text x={6} y={top + 11}>{format(max)}</text>
              <text x={6} y={top + plot / 2 + 4}>{format(max / 2)}</text>
            </g>
          )}
          {shownEvents.map((event, index) => {
            const at = x(event.atMs);
            if (at < 0 || at > width) return null;
            const problem = isProblem(event);
            return (
              <line
                key={`${event.atMs}-${index}`}
                x1={at}
                x2={at}
                y1={height - band + 3}
                y2={height - 1}
                stroke={problem ? "var(--m-danger)" : "var(--text-3)"}
                strokeWidth={problem ? 2 : 1}
                opacity={problem ? 0.95 : 0.5}
              />
            );
          })}
          {hover !== null && (
            <line x1={x(hover)} x2={x(hover)} y1={top} y2={top + plot} stroke="var(--text-3)" strokeDasharray="3 3" />
          )}
          {selectedShown && selected !== null && (
            <g>
              <line
                x1={x(selected)}
                x2={x(selected)}
                y1={top}
                y2={top + plot}
                stroke="var(--accent)"
                strokeWidth={5}
                opacity={0.16}
              />
              <line x1={x(selected)} x2={x(selected)} y1={top} y2={top + plot} stroke="var(--accent)" strokeWidth={1.5} />
              {picked && (
                <circle cx={x(selected)} cy={y(picked.value)} r={3.5} fill="var(--accent)" stroke="var(--surface)" strokeWidth={1.5} />
              )}
            </g>
          )}
        </svg>
        {widest !== null && widest.width > 110 && (
          <span className="timeline-blank" style={{ left: (widest.left + widest.right) / 2 }}>
            {t("history.noRecord")}
          </span>
        )}
        {loading && <span className="timeline-blank">…</span>}
        {hover !== null && (
          <div className="timeline-tip" style={{ left: clamp(hover, 62) }}>
            <span className="num">{label(hover)}</span>
            {readout !== null && <strong className="num">{format(readout)}</strong>}
          </div>
        )}
        {selectedShown && selected !== null && (
          <div className="timeline-pin" style={{ left: clamp(selected, 76) }} onClick={(event) => event.stopPropagation()}>
            <span className="num">{label(selected)}</span>
            {picked && <strong className="num">{format(picked.value)}</strong>}
            <button
              className="timeline-pin-x"
              aria-label={t("history.clearSelection")}
              title={t("history.clearSelection")}
              onClick={() => onSelect(null)}
            >
              ×
            </button>
          </div>
        )}
      </div>
      <div className="timeline-axis">
        <span className="num">{label(view.fromMs)}</span>
        <span className="num">{label(view.toMs)}</span>
      </div>
    </div>
  );
}

/**
 * What was running at the chosen moment, and what Windows logged around it. The heading carries
 * the way out of the selection: a moment picked on the chart used to be permanent until the tab
 * was left, and nothing in the panel said it could be dropped.
 */
function MomentPanel({
  at,
  range,
  events,
  onClear,
}: {
  at: number;
  range: number;
  events: SystemEvent[];
  onClear: () => void;
}) {
  const { t, units, openDetail } = useStore();
  const apps = useLoaded(() => ipc.appsAt(at), [] as AppUsage[], [at]);
  const reach = range <= 86_400 ? 15 * 60_000 : 60 * 60_000;
  const nearby = events.filter((e) => Math.abs(e.atMs - at) <= reach);
  const peak = Math.max(1e-9, ...apps.map((a) => a.cpuSeconds / Math.max(1, a.seconds)));
  return (
    <div className="moment">
      <div className="moment-head">
        <h4 className="section-title">{t("history.moment", { time: formatWhen(at, range, units.locale) })}</h4>
        <Button variant="chip" className="moment-clear" onClick={onClear}>
          {t("history.clearSelection")}
        </Button>
      </div>
      {apps.length === 0 ? (
        <p className="empty faint">{t("history.momentEmpty")}</p>
      ) : (
        <div className="rows">
          {apps.slice(0, 8).map((app) => {
            const cpu = (app.cpuSeconds / Math.max(1, app.seconds)) * 100;
            const label = appLabel(app, t);
            return (
              <div
                key={app.key}
                className="row clickable"
                role="button"
                tabIndex={0}
                onClick={() => openDetail(app.key, seedOf(app))}
                onKeyDown={(event) => event.key === "Enter" && openDetail(app.key, seedOf(app))}
              >
                <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
                <div className="row-main">
                  <span className="row-name">{label}</span>
                  <span className="row-sub">
                    {formatBytes(app.memoryAverage, units)}
                    {app.writeBytes > 0 ? ` · ${t("history.written", { value: formatBytes(app.writeBytes, units) })}` : ""}
                  </span>
                </div>
                <div className="row-meter">
                  <Meter ratio={cpu / 100 / peak} color={TONE.cpu} />
                </div>
                <span className="row-value num">{formatPercent(cpuDisplay(cpu, units), units.locale, cpu >= 10 ? 0 : 1)}</span>
              </div>
            );
          })}
        </div>
      )}
      {nearby.length > 0 && (
        <>
          <h4 className="section-title">{t("history.nearby")}</h4>
          <EventList events={nearby} range={range} />
        </>
      )}
    </div>
  );
}

/** The heaviest apps over the range, by the measure the timeline shows. */
function RankingCard({ range, by }: { range: number; by: RankBy }) {
  const { t, units, openDetail } = useStore();
  const ranking = useLoaded(() => ipc.appRanking(range, by), [] as AppUsage[], [range, by]);
  const measure = (app: AppUsage) =>
    by === "cpu" ? app.cpuSeconds : by === "memory" ? app.memoryAverage : by === "write" ? app.writeBytes : app.gpuSeconds;
  const peak = Math.max(1e-9, ...ranking.map(measure));
  const describe = (app: AppUsage): [string, string] => {
    const seen = t("history.seenFor", { value: formatDuration(app.seconds, units.locale, { short: true }) });
    switch (by) {
      case "cpu":
        return [t("history.cpuTime", { value: formatDuration(app.cpuSeconds, units.locale, { short: true }) }), seen];
      case "memory":
        return [
          t("history.avgMemory", { avg: formatBytes(app.memoryAverage, units), peak: formatBytes(app.memoryPeak, units) }),
          seen,
        ];
      case "write":
      case "disk":
        return [t("history.written", { value: formatBytes(app.writeBytes, units) }), seen];
      case "gpu":
        return [t("history.gpuTime", { value: formatDuration(app.gpuSeconds, units.locale, { short: true }) }), seen];
    }
  };
  return (
    <Card title={t("history.ranking")}>
      {ranking.length === 0 ? (
        <p className="empty faint">{t("history.rankingEmpty")}</p>
      ) : (
        <div className="rows">
          {ranking.slice(0, 15).map((app) => {
            const [value, sub] = describe(app);
            const label = appLabel(app, t);
            return (
              <div
                key={app.key}
                className="row clickable ranked"
                role="button"
                tabIndex={0}
                onClick={() => openDetail(app.key, seedOf(app))}
                onKeyDown={(event) => event.key === "Enter" && openDetail(app.key, seedOf(app))}
              >
                <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
                <div className="row-main">
                  <span className="row-name">{label}</span>
                  <span className="row-sub">
                    {t(`kind.${app.kind}`)} · {sub}
                  </span>
                </div>
                <div className="row-meter">
                  <Meter ratio={measure(app) / peak} color="var(--accent)" />
                </div>
                <span className="row-value num history-value">{value}</span>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}

function EventList({ events, range, onPick }: { events: SystemEvent[]; range: number; onPick?: (at: number) => void }) {
  const { t, units } = useStore();
  if (events.length === 0) return <p className="empty faint">{t("history.eventsEmpty")}</p>;
  return (
    <div className="rows">
      {events.slice(0, 80).map((event, index) => (
        <div
          key={`${event.atMs}-${index}`}
          className={`event-row ${onPick ? "clickable" : ""}`}
          role={onPick ? "button" : undefined}
          tabIndex={onPick ? 0 : undefined}
          onClick={() => onPick?.(event.atMs)}
          onKeyDown={(e) => e.key === "Enter" && onPick?.(event.atMs)}
        >
          <span className={`event-dot ${isProblem(event) ? "problem" : ""}`} />
          <span className="event-time num">{formatWhen(event.atMs, Math.max(range, 86_401), units.locale)}</span>
          <span className="event-text">{eventText(event, t)}</span>
        </div>
      ))}
    </div>
  );
}
