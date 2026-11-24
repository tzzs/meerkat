import type { Key } from "../i18n/en";
import { formatBytes, formatPercent, formatTemp, type UnitPrefs } from "./format";
import type {
  AppInfo,
  AppKind,
  PerfIssue,
  PerfReport,
  PerfSample,
  PerfSession,
  PerfStat,
  PerfTarget,
  PerfVerdict,
} from "./types";

export type PerfUnit = "fps" | "percent" | "celsius" | "bytes";

export type PerfMetricKey =
  | "fps"
  | "cpuPercent"
  | "cpuTempC"
  | "gpuPercent"
  | "gpuTempC"
  | "memoryPercent"
  | "vramPercent"
  | "vramUsedBytes";

export interface PerfMetric {
  key: PerfMetricKey;
  labelKey: Key;
  unit: PerfUnit;
  tone: string;
  ceiling?: number;
  read: (sample: PerfSample) => number | null;
}

/** One table owns the eight readings, their units, their labels and their place in the UI. */
export const PERF_METRICS: readonly PerfMetric[] = [
  { key: "fps", labelKey: "perf.metric.fps", unit: "fps", tone: "var(--accent)", ceiling: 120, read: (s) => s.fps },
  {
    key: "cpuPercent",
    labelKey: "perf.metric.cpu",
    unit: "percent",
    tone: "var(--m-cpu)",
    ceiling: 100,
    read: (s) => s.cpuPercent,
  },
  {
    key: "cpuTempC",
    labelKey: "perf.metric.cpuTemp",
    unit: "celsius",
    tone: "var(--m-temp)",
    ceiling: 110,
    read: (s) => s.cpuTempC,
  },
  {
    key: "gpuPercent",
    labelKey: "perf.metric.gpu",
    unit: "percent",
    tone: "var(--m-gpu)",
    ceiling: 100,
    read: (s) => s.gpuPercent,
  },
  {
    key: "gpuTempC",
    labelKey: "perf.metric.gpuTemp",
    unit: "celsius",
    tone: "var(--m-temp)",
    ceiling: 110,
    read: (s) => s.gpuTempC,
  },
  {
    key: "memoryPercent",
    labelKey: "perf.metric.memory",
    unit: "percent",
    tone: "var(--m-memory)",
    ceiling: 100,
    read: (s) => s.memoryPercent,
  },
  {
    key: "vramPercent",
    labelKey: "perf.metric.vram",
    unit: "percent",
    tone: "var(--m-gpu)",
    ceiling: 100,
    read: (s) => s.vramPercent,
  },
  {
    key: "vramUsedBytes",
    labelKey: "perf.metric.vramBytes",
    unit: "bytes",
    tone: "var(--m-gpu)",
    read: (s) => s.vramUsedBytes,
  },
];

export function metricOf(key: PerfMetricKey): PerfMetric {
  return PERF_METRICS.find((metric) => metric.key === key) as PerfMetric;
}

function seriesFrom(samples: readonly PerfSample[], key: PerfMetricKey): number[] {
  const metric = metricOf(key);
  const values: number[] = [];
  for (const sample of samples) {
    const value = metric.read(sample);
    if (value !== null && Number.isFinite(value)) values.push(value);
  }
  return values;
}

export function perfSeries(session: PerfSession, key: PerfMetricKey): number[] {
  return seriesFrom(session.samples, key);
}

export function perfSampleSeries(samples: readonly PerfSample[], key: PerfMetricKey): number[] {
  return seriesFrom(samples, key);
}

/** A dash is a reading nobody took, never a zero dressed up as one. */
export function formatPerfValue(value: number | null | undefined, unit: PerfUnit, units: UnitPrefs): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  switch (unit) {
    case "fps":
      return `${new Intl.NumberFormat(units.locale, { maximumFractionDigits: 0 }).format(value)} fps`;
    case "percent":
      return formatPercent(value, units.locale, 0);
    case "celsius":
      return formatTemp(value, units);
    case "bytes":
      return formatBytes(value, units);
  }
}

export function formatPerfStat(stat: PerfStat | undefined, unit: PerfUnit, units: UnitPrefs): string {
  if (!stat || !stat.available) return "—";
  return formatPerfValue(stat.last, unit, units);
}

const VERDICTS: readonly PerfVerdict[] = ["healthy", "warning", "problem", "inconclusive"];
const ISSUE_KINDS: readonly PerfIssue["kind"][] = [
  "fpsLow",
  "fpsUnstable",
  "cpuBound",
  "gpuBound",
  "vramPressure",
  "gpuHot",
  "memoryPressure",
  "fpsUnavailable",
];

export function perfVerdictKey(verdict: PerfVerdict): Key {
  return `perf.verdict.${verdict}` as Key;
}

export function perfHeadlineKey(report: PerfReport): Key {
  const key = report.headlineKey;
  return (
    key.startsWith("perf.headline.") && VERDICTS.some((v) => key === `perf.headline.${v}`)
      ? key
      : `perf.headline.${report.verdict}`
  ) as Key;
}

export function perfIssueKey(issue: PerfIssue): Key {
  const key = issue.messageKey;
  return (
    ISSUE_KINDS.some((kind) => key === `perf.issue.${kind}`) ? key : `perf.issue.${issue.kind}`
  ) as Key;
}

/** The number beside a finding uses the finding's own scale when the backend metric is unknown. */
export function perfIssueValue(issue: PerfIssue, units: UnitPrefs): string {
  const metric = PERF_METRICS.find((entry) => entry.key === issue.metric);
  const unit: PerfUnit =
    metric?.unit ??
    (issue.kind === "gpuHot" ? "celsius" : issue.kind.startsWith("fps") ? "fps" : "percent");
  return formatPerfValue(issue.value, unit, units);
}

/** A blank name means the backend had no window to name, so the caller's mode label takes over. */
export function perfTargetLabel(target: Pick<PerfTarget, "mode" | "name">, fallback: string): string {
  return target.name.trim() || fallback;
}

/** Only the operating system's services and system processes are not worth monitoring. */
const PICKABLE_KINDS: ReadonlySet<AppKind> = new Set<AppKind>(["app", "background", "packaged"]);

/**
 * The apps the picker offers, filtered by a name or executable path. A session is about one
 * program, so the service hosts and system processes the rest of the window lists stay out.
 */
export function perfAppChoices<T extends Pick<AppInfo, "name" | "kind" | "exePath">>(
  apps: readonly T[],
  query: string,
): T[] {
  const needle = query.trim().toLowerCase();
  return apps
    .filter((app) => PICKABLE_KINDS.has(app.kind) && app.name.trim() !== "")
    .filter(
      (app) =>
        needle === "" ||
        app.name.toLowerCase().includes(needle) ||
        app.exePath.toLowerCase().includes(needle),
    )
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * What the app picker's rows actually show, as one string.
 *
 * The picker is rebuilt when this changes, and only then. The snapshot object itself is new
 * every second, so keying the list on it would hand the rows a fresh array on every tick for
 * figures they do not draw — which is what made the Performance tab drop a frame a second and
 * stutter when its list was scrolled. `processCount` is part of it on purpose: a row that said
 * "2 processes" while the app ran five would be a stale reading, not a stable one.
 */
export function pickerSignature(
  apps: readonly Pick<AppInfo, "key" | "name" | "exePath" | "kind" | "processCount">[],
): string {
  return apps
    .map((app) => `${app.key}\u0000${app.name}\u0000${app.exePath}\u0000${app.kind}\u0000${app.processCount}`)
    .join("\u0001");
}
