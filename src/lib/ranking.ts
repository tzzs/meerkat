import type { RankedApp, RankingMetric, RankingRow } from "./types";

export type { RankingMetric };

export interface HourRanking {
  hourStartMs: number;
  /** The busiest few of that hour by the chosen metric, largest first. */
  apps: RankedApp[];
}

/**
 * The hourly record, regrouped for one metric: a row per hour, newest first, holding that
 * hour's `perHour` busiest apps.
 *
 * `history_ranking` answers with a row per hour and metric (memory's top forty, CPU's top ten),
 * ascending in time. The list wants the opposite end first, one metric at a time, and only the
 * few apps worth a glance. An app that used nothing that hour is not "among the busiest", and
 * an hour left with nobody in it is a gap in the record, not a row of its own.
 */
export function rankingByHour(rows: readonly RankingRow[], metric: RankingMetric, perHour = 3): HourRanking[] {
  const hours = new Map<number, Map<string, RankedApp>>();
  for (const row of rows) {
    if (row.metric !== metric || !Number.isFinite(row.hourStartMs)) continue;
    const apps = hours.get(row.hourStartMs) ?? new Map<string, RankedApp>();
    for (const app of row.apps) {
      if (!Number.isFinite(app.value) || app.value <= 0) continue;
      const known = apps.get(app.key);
      if (!known || known.value < app.value) apps.set(app.key, app);
    }
    hours.set(row.hourStartMs, apps);
  }
  return [...hours.entries()]
    .map(([hourStartMs, apps]) => ({
      hourStartMs,
      apps: [...apps.values()].sort((a, b) => b.value - a.value).slice(0, perHour),
    }))
    .filter((hour) => hour.apps.length > 0)
    .sort((a, b) => b.hourStartMs - a.hourStartMs);
}

/**
 * The executable to ask for an icon, from an app's key. A key is the main executable's path,
 * lowercased, with `|role` after it for a service host; anything that is not a path (the row of
 * unreadable processes) has no icon to ask for and gets its initial instead.
 */
export function iconPathForKey(key: string): string | undefined {
  const path = key.split("|")[0];
  return /^[a-z]:[\\/]/i.test(path) || path.startsWith("\\\\") ? path : undefined;
}

function dayKey(atMs: number, timeZone?: string): string {
  return new Intl.DateTimeFormat("en-CA", { year: "numeric", month: "2-digit", day: "2-digit", timeZone }).format(atMs);
}

/**
 * "14:00" for an hour of today, with the weekday in front of it for an earlier day. The list
 * spans two days, and a bare clock time says nothing about which of them it belongs to.
 */
export function formatHourLabel(hourStartMs: number, nowMs: number, locale: string, timeZone?: string): string {
  const clock: Intl.DateTimeFormatOptions = { hour: "2-digit", minute: "2-digit", timeZone };
  const sameDay = dayKey(hourStartMs, timeZone) === dayKey(nowMs, timeZone);
  return new Intl.DateTimeFormat(locale, sameDay ? clock : { weekday: "short", ...clock }).format(hourStartMs);
}
