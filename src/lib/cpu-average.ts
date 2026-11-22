import { useEffect, useRef, useState } from "react";
import type { AppInfo } from "./types";

/** Five seconds of one-second samples: long enough to see a busy app, short enough to still move. */
export const CPU_AVERAGE_SAMPLES = 5;

/**
 * Each app's CPU over the last `window` readings. The caller owns `samples`; this fills it for the
 * apps in front of it, trims each list, drops the apps that are gone, and returns the means.
 */
export function cpuAverages(
  samples: Map<string, number[]>,
  apps: readonly AppInfo[],
  window = CPU_AVERAGE_SAMPLES,
): Map<string, number> {
  const averages = new Map<string, number>();
  const live = new Set<string>();
  for (const app of apps) {
    live.add(app.key);
    const list = samples.get(app.key) ?? [];
    list.push(app.cpuPercent);
    if (list.length > window) list.splice(0, list.length - window);
    samples.set(app.key, list);
    averages.set(app.key, list.reduce((sum, value) => sum + value, 0) / list.length);
  }
  for (const key of [...samples.keys()]) {
    if (!live.has(key)) samples.delete(key);
  }
  return averages;
}

/**
 * A CPU reading per app that does not jump with every sample. The samples live in a ref, so a
 * new snapshot averages into the same window instead of restarting it.
 */
export function useCpuAverage(apps: readonly AppInfo[]): ReadonlyMap<string, number> {
  const samples = useRef(new Map<string, number[]>());
  const sampled = useRef<readonly AppInfo[] | null>(null);
  // One reading is a mean of one sample, so the first render has an answer before any effect runs.
  const [averages, setAverages] = useState<ReadonlyMap<string, number>>(
    () => new Map(apps.map((app) => [app.key, app.cpuPercent])),
  );
  useEffect(() => {
    // One sample per array: StrictMode runs the effect twice on mount, and a reading recorded
    // twice counts double in the mean.
    if (sampled.current === apps) return;
    sampled.current = apps;
    setAverages(cpuAverages(samples.current, apps));
  }, [apps]);
  return averages;
}
