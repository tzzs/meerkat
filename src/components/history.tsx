import { useEffect, useMemo, useRef, useState } from "react";
import { useLoaded } from "../lib/use-loaded";
import { cpuDisplay, formatBytes, formatDuration, formatPercent, formatRate } from "../lib/format";
import { ipc } from "../lib/ipc";
import { sectionShown, useStore, visibleSections, type AppSeed } from "../lib/store";
import { appLabel } from "../lib/apps";
import { eventText, isProblem } from "../lib/events";
import type { AppUsage, HistoryPoint, RankBy, SystemEvent } from "../lib/types";
import type { Key } from "../i18n/en";
import { AppIcon, Card, Meter } from "./ui";
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

  const pick = (at: number) => {
    setMoment(at);
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
            format={(value) =>
              measure.percent ? formatPercent(value, units.locale) : formatRate(value, units, "storage")
            }
            fixedMax={measure.percent ? 100 : undefined}
            events={events}
            selected={moment}
            onSelect={setMoment}
            label={(at) => formatWhen(at, range, units.locale)}
          />
          <p className="hint faint">{t("history.clickHint")}</p>
          {moment !== null && <MomentPanel at={moment} range={range} events={events} />}
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
 * describes. Gaps where Pulse was not running are left as gaps rather than joined across.
 */
function TimelineChart({
  points,
  bucketSeconds,
  from,
  to,
  tone,
  format,
  fixedMax,
  events,
  selected,
  onSelect,
  label,
}: {
  points: HistoryPoint[];
  bucketSeconds: number;
  from: number;
  to: number;
  tone: string;
  format: (value: number) => string;
  fixedMax?: number;
  events: SystemEvent[];
  selected: number | null;
  onSelect: (at: number) => void;
  label: (at: number) => string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(720);
  const [hover, setHover] = useState<number | null>(null);
  useEffect(() => {
    const element = box.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setWidth(Math.max(200, entry.contentRect.width)));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const height = 168;
  const band = 16;
  const top = 8;
  const plot = height - band - top;
  const span = Math.max(1, to - from);
  const visible = points.filter((p) => p.atMs >= from && p.atMs <= to);
  const max = fixedMax ?? Math.max(1, ...visible.map((p) => p.value)) * 1.15;
  const x = (at: number) => ((at - from) / span) * width;
  const y = (value: number) => top + plot - (Math.min(value, max) / max) * plot;

  const segments = useMemo(() => {
    const out: HistoryPoint[][] = [];
    let current: HistoryPoint[] = [];
    for (const point of visible) {
      const previous = current[current.length - 1];
      if (previous && point.atMs - previous.atMs > bucketSeconds * 1000 * 2.5) {
        out.push(current);
        current = [];
      }
      current.push(point);
    }
    if (current.length) out.push(current);
    return out;
  }, [visible, bucketSeconds]);

  const nearest = (at: number) =>
    visible.reduce<HistoryPoint | null>(
      (best, p) => (best === null || Math.abs(p.atMs - at) < Math.abs(best.atMs - at) ? p : best),
      null,
    );
  const atOf = (clientX: number) => {
    const rect = box.current?.getBoundingClientRect();
    if (!rect) return from;
    return from + ((clientX - rect.left) / rect.width) * span;
  };
  const hovered = hover === null ? null : nearest(hover);

  return (
    <div className="timeline">
      <div
        ref={box}
        className="timeline-plot"
        onPointerMove={(event) => setHover(atOf(event.clientX))}
        onPointerLeave={() => setHover(null)}
        onClick={(event) => onSelect(Math.round(atOf(event.clientX)))}
      >
        <svg width={width} height={height} role="img" aria-label={label(from) + " – " + label(to)}>
          <line x1={0} x2={width} y1={top + plot} y2={top + plot} stroke="var(--hairline-strong)" />
          {segments.map((segment, index) => {
            const line = segment.map((p, i) => `${i === 0 ? "M" : "L"}${x(p.atMs).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ");
            const area =
              segment.length > 1
                ? `${line} L${x(segment[segment.length - 1].atMs).toFixed(1)},${top + plot} L${x(segment[0].atMs).toFixed(1)},${top + plot} Z`
                : "";
            return (
              <g key={index}>
                {area && <path d={area} fill={tone} opacity={0.14} />}
                <path d={line} fill="none" stroke={tone} strokeWidth={1.5} strokeLinejoin="round" />
              </g>
            );
          })}
          {events.map((event, index) => {
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
          {selected !== null && selected >= from && selected <= to && (
            <line x1={x(selected)} x2={x(selected)} y1={top} y2={height} stroke="var(--accent)" strokeWidth={1.5} />
          )}
          {hover !== null && (
            <line x1={x(hover)} x2={x(hover)} y1={top} y2={top + plot} stroke="var(--text-3)" strokeDasharray="3 3" />
          )}
        </svg>
        {hover !== null && (
          <div className="timeline-tip" style={{ left: Math.min(Math.max(x(hover), 60), width - 60) }}>
            <span className="num">{label(hover)}</span>
            {hovered && Math.abs(hovered.atMs - hover) < bucketSeconds * 1000 * 1.5 && (
              <strong className="num">{format(hovered.value)}</strong>
            )}
          </div>
        )}
      </div>
      <div className="timeline-axis">
        <span className="num">{label(from)}</span>
        <span className="num">{label(to)}</span>
      </div>
    </div>
  );
}

/** What was running at the chosen moment, and what Windows logged around it. */
function MomentPanel({ at, range, events }: { at: number; range: number; events: SystemEvent[] }) {
  const { t, units, openDetail } = useStore();
  const apps = useLoaded(() => ipc.appsAt(at), [] as AppUsage[], [at]);
  const reach = range <= 86_400 ? 15 * 60_000 : 60 * 60_000;
  const nearby = events.filter((e) => Math.abs(e.atMs - at) <= reach);
  const peak = Math.max(1e-9, ...apps.map((a) => a.cpuSeconds / Math.max(1, a.seconds)));
  return (
    <div className="moment">
      <h4 className="section-title">{t("history.moment", { time: formatWhen(at, range, units.locale) })}</h4>
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
