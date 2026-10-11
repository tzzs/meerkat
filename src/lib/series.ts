import { formatBytes, formatDay, formatPercent, formatRate, formatTemp, type UnitPrefs } from "./format";
import type { HistoryPoint } from "./types";

/** One drawable sample: x is a fraction of the time window, values are in the metric's unit. */
export interface SeriesPoint {
  x: number;
  mean: number;
  max: number;
}

export interface BuiltSeries {
  /** Runs of points with no gap between them; a gap is drawn as a gap. */
  segments: SeriesPoint[][];
  /** One bucket's width as a fraction of the window, so a lone point can be drawn that wide. */
  bucketFraction: number;
  /** Lowest and average bucket mean, and the highest bucket maximum; null with no data. */
  stats: { min: number; avg: number; peak: number } | null;
}

/** Neighbours further apart than this many buckets are separated by missing data. */
export const GAP_BUCKETS = 2.5;

/** The backend names its own bucket; one that does not is read as the finest it keeps. */
function bucketMilliseconds(bucketSeconds: number): number {
  return (Number.isFinite(bucketSeconds) && bucketSeconds > 0 ? bucketSeconds : 60) * 1000;
}

/**
 * A history series placed on a real time axis, [fromMs, toMs].
 *
 * The chart used to be handed the values alone and spaced them by index, so every gap - the
 * machine asleep, Pulse not running - closed up: this machine's twelve hours had a 195-minute
 * hole drawn as a continuous line, and "last 7 days" was only as wide as the days that had
 * data. Points outside the window are dropped, and a run is broken wherever two neighbours
 * are more than GAP_BUCKETS buckets apart.
 */
export function buildSeries(
  points: readonly HistoryPoint[],
  fromMs: number,
  toMs: number,
  bucketSeconds: number,
): BuiltSeries {
  const span = toMs - fromMs;
  const bucketMs = bucketMilliseconds(bucketSeconds);
  if (!(span > 0)) return { segments: [], bucketFraction: 0, stats: null };
  const inside = points
    .filter((p) => p.atMs >= fromMs && p.atMs <= toMs && Number.isFinite(p.value))
    .sort((a, b) => a.atMs - b.atMs);

  const segments: SeriesPoint[][] = [];
  let run: SeriesPoint[] = [];
  let previous: number | null = null;
  let sum = 0;
  let min = Infinity;
  let peak = -Infinity;
  for (const p of inside) {
    if (previous !== null && p.atMs - previous > GAP_BUCKETS * bucketMs) {
      segments.push(run);
      run = [];
    }
    const max = p.secondary !== null && Number.isFinite(p.secondary) ? Math.max(p.secondary, p.value) : p.value;
    run.push({ x: (p.atMs - fromMs) / span, mean: p.value, max });
    previous = p.atMs;
    sum += p.value;
    min = Math.min(min, p.value);
    peak = Math.max(peak, max);
  }
  if (run.length > 0) segments.push(run);
  return {
    segments,
    bucketFraction: bucketMs / span,
    stats: inside.length > 0 ? { min, avg: sum / inside.length, peak } : null,
  };
}

export interface SeriesPaths {
  /** The bucket means, one subpath per run. */
  line: string;
  /** The area under the means. */
  area: string;
  /** The area under the bucket maxima: the envelope drawn lightly behind the mean. */
  envelope: string;
}

/**
 * A run of one bucket as a short flat stub that stays inside the box: a zero-length line is
 * invisible, and that bucket really was measured. At the right edge, where "now" is, it grows
 * leftwards instead of off the end.
 */
function stub(p: SeriesPoint, widthFraction: number): SeriesPoint[] {
  const end = Math.min(1, p.x + widthFraction);
  return [
    { ...p, x: Math.max(0, end - widthFraction) },
    { ...p, x: end },
  ];
}

/**
 * SVG paths for a built series in a `width` × `height` box with the given ceiling. A run of
 * one point is drawn one bucket wide (at least two units).
 */
export function seriesPaths(built: BuiltSeries, width: number, height: number, ceiling: number): SeriesPaths {
  const top = Number.isFinite(ceiling) && ceiling > 0 ? ceiling : 1;
  const y = (v: number) => (height - Math.min(1, Math.max(0, v / top)) * height).toFixed(2);
  const x = (fraction: number) => (Math.min(1, Math.max(0, fraction)) * width).toFixed(2);
  const lone = Math.max(2 / width, built.bucketFraction);

  let line = "";
  let area = "";
  let envelope = "";
  for (const run of built.segments) {
    const drawn = run.length === 1 ? stub(run[0], lone) : run;
    const first = x(drawn[0].x);
    const last = x(drawn[drawn.length - 1].x);
    const means = drawn.map((p) => `${x(p.x)},${y(p.mean)}`);
    const maxima = drawn.map((p) => `${x(p.x)},${y(p.max)}`);
    line += `M${means.join(" L")} `;
    area += `M${first},${height} L${means.join(" L")} L${last},${height} Z `;
    envelope += `M${first},${height} L${maxima.join(" L")} L${last},${height} Z `;
  }
  return { line: line.trim(), area: area.trim(), envelope: envelope.trim() };
}

/** What a history metric is measured in, which decides its scale and its words. */
export type HistoryUnit = "percent" | "bytes" | "storageRate" | "networkRate" | "celsius";

/** The unit of a `history_series` metric id, or null for one this build does not know. */
export function historyUnit(metric: string): HistoryUnit | null {
  switch (metric) {
    case "cpu":
    case "memory":
    case "gpu":
    case "battery":
      return "percent";
    case "memoryUsed":
    case "memoryTotal":
    case "available":
    case "committed":
      return "bytes";
    case "diskRead":
    case "diskWrite":
      return "storageRate";
    case "download":
    case "upload":
      return "networkRate";
    case "temp":
      return "celsius";
    default:
      return null;
  }
}

/**
 * The top of a history plot. A percentage is plotted against 100 and a temperature against the
 * 110 degrees the sensor meters use, so a quiet machine draws a quiet line; fitted to its own
 * data, an idle 3% filled the card. A rate has no natural maximum and is fitted to the window's
 * own peak, with a little headroom so the line does not run along the frame.
 */
export function historyCeiling(unit: HistoryUnit | null, peak: number): number {
  const observed = Number.isFinite(peak) && peak > 0 ? peak : 0;
  if (unit === "percent") return Math.max(100, observed);
  if (unit === "celsius") return Math.max(110, observed);
  return observed > 0 ? observed * 1.05 : 1;
}

/** One figure of a history series, in the unit and in the user's units. */
export function formatHistoryValue(metric: string, value: number, units: UnitPrefs): string {
  switch (historyUnit(metric)) {
    case "percent":
      // A quiet machine lives below 10%, where whole numbers would round everything to 0–9.
      return formatPercent(value, units.locale, value > 0 && value < 10 ? 1 : 0);
    case "bytes":
      return formatBytes(value, units);
    case "storageRate":
      return formatRate(value, units, "storage");
    case "networkRate":
      return formatRate(value, units, "network");
    case "celsius":
      return formatTemp(value, units);
    default:
      return new Intl.NumberFormat(units.locale, { maximumFractionDigits: 1 }).format(value);
  }
}

/**
 * The label at the left edge of a history plot. Within half a day the clock is enough; a day
 * needs the weekday to say which one; beyond that the date is the useful part.
 */
export function formatAxisTime(atMs: number, rangeSeconds: number, locale: string, timeZone?: string): string {
  if (rangeSeconds > 86400) return formatDay(atMs, locale, timeZone);
  const clock: Intl.DateTimeFormatOptions = { hour: "2-digit", minute: "2-digit", timeZone };
  return new Intl.DateTimeFormat(
    locale,
    rangeSeconds > 12 * 3600 ? { weekday: "short", ...clock } : clock,
  ).format(atMs);
}
