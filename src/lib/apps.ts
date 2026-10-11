import type { Translate } from "../i18n";
import { UNREADABLE_GROUP, type AppInfo } from "./types";

export type AppMetric = "cpu" | "memory" | "disk" | "network" | "connections";

export function appValue(app: AppInfo, metric: AppMetric): number {
  switch (metric) {
    case "cpu":
      return app.cpuPercent;
    case "memory":
      return app.memoryBytes;
    case "disk":
      return app.readBytesPerSecond + app.writeBytesPerSecond;
    case "network":
    case "connections":
      return app.connections;
  }
}

/**
 * Pinned apps first, in the order they were pinned; then everything else by the metric,
 * largest first; ties keep the order they arrived in.
 *
 * The comparator this replaces mapped "not pinned" to 1, the same number as the second pin,
 * so only the first pin reliably reached the top: pinning D and E over A–F gave "D A B C E F".
 */
export function orderApps(apps: readonly AppInfo[], pinned: readonly string[], metric: AppMetric): AppInfo[] {
  const pinAt = new Map(pinned.map((key, index) => [key, index]));
  return apps
    .map((app, index) => ({ app, index, pin: pinAt.get(app.key), value: appValue(app, metric) }))
    .sort((a, b) => {
      if (a.pin !== undefined || b.pin !== undefined) {
        if (a.pin === undefined) return 1;
        if (b.pin === undefined) return -1;
        return a.pin - b.pin;
      }
      const byValue = b.value - a.value;
      return byValue !== 0 && !Number.isNaN(byValue) ? byValue : a.index - b.index;
    })
    .map((entry) => entry.app);
}

/**
 * The rows in a held order. While the pointer is over a list its rows keep their places and
 * only their figures change; re-sorting every second slid a different app under the cursor
 * just as the × was about to be clicked. Apps that have gone drop out, and new ones join at
 * the end rather than pushing the rest down.
 */
export function holdOrder(sorted: readonly AppInfo[], held: readonly string[] | null): AppInfo[] {
  if (!held) return [...sorted];
  const byKey = new Map(sorted.map((app) => [app.key, app]));
  const out: AppInfo[] = [];
  const placed = new Set<string>();
  for (const key of held) {
    const app = byKey.get(key);
    if (app && !placed.has(key)) {
      out.push(app);
      placed.add(key);
    }
  }
  for (const app of sorted) if (!placed.has(app.key)) out.push(app);
  return out;
}

/** svchost groups are keyed `<path>\svchost.exe|<role>`; this is the role, or null. */
export function serviceHostRole(key: string): string | null {
  const match = /(?:^|[\\/])svchost\.exe\|(.+)$/i.exec(key);
  return match ? match[1] : null;
}

/**
 * The name a row shows. The backend names a service host in English ("Service host: netsvcs")
 * and the folded row of unreadable processes not at all, so both are worded here instead.
 */
export function appLabel(app: Pick<AppInfo, "key" | "name">, t: Translate): string {
  if (app.key === UNREADABLE_GROUP) return t("list.unreadable");
  const role = serviceHostRole(app.key);
  if (role) return `${t("label.serviceHost")}: ${role}`;
  return app.name;
}

/** The executable's file name ("chrome.exe"), or the app's name when Windows gave no path. */
export function processName(app: Pick<AppInfo, "exePath" | "name">): string {
  return app.exePath.split(/[\\/]/).pop() || app.name;
}

/**
 * Whether Pulse offers to close it. Windows' own processes are not something to end from a
 * monitor, and the unreadable row is a count, not an application.
 */
export function canQuit(app: Pick<AppInfo, "key" | "kind">): boolean {
  return app.key !== UNREADABLE_GROUP && app.kind !== "system";
}

/** The titles worth showing on hover: windows a person can actually see, once each. */
export function visibleWindowTitles(app: Pick<AppInfo, "windows">): string[] {
  const titles = app.windows.filter((w) => w.visible && w.title.trim()).map((w) => w.title.trim());
  return [...new Set(titles)];
}

/** "Show Windows in lists" off hides the operating system's own processes and services. */
export function forLists(apps: readonly AppInfo[], showSystem: boolean): AppInfo[] {
  return showSystem ? [...apps] : apps.filter((app) => app.kind !== "system" && app.kind !== "service");
}
