import { squarify, type Rect } from "./storage";
import type { AppInfo } from "./types";

/** Enough boxes to name the programs people recognise without the tail turning into slivers. */
export const MAX_BOXES = 20;
/** Below this share of the measured total an app is folded into the "other" box. */
export const MIN_SHARE = 0.005;

export type TreemapBox =
  | { kind: "app"; value: number; share: number; rect: Rect; app: AppInfo }
  | { kind: "other"; value: number; share: number; rect: Rect; apps: AppInfo[] };

export interface TreemapData {
  boxes: TreemapBox[];
  mappedValue: number;
}

/**
 * The apps, as boxes sized by whatever `valueOf` measures. The biggest few are named; everything
 * below `MIN_SHARE`, and anything past `MAX_BOXES`, is folded into one box so the map still covers
 * every byte and every percent Meerkat measured.
 */
export function treemap(apps: readonly AppInfo[], valueOf: (app: AppInfo) => number): TreemapData {
  // Read through `valueOf` once per app: a comparator would call it O(n log n) times.
  const measured = apps
    .map((app) => ({ app, value: valueOf(app) }))
    .filter((entry) => Number.isFinite(entry.value) && entry.value > 0);
  const mappedValue = measured.reduce((sum, entry) => sum + entry.value, 0);
  if (!(mappedValue > 0)) return { boxes: [], mappedValue: 0 };

  // `sort` is stable, so equal readings keep the order the sampler sent them in.
  const sorted = [...measured].sort((a, b) => b.value - a.value);
  // The list is sorted, so the apps that clear the floor are a prefix: the fold can start
  // where the named ones end.
  const clearing = sorted.filter((entry) => entry.value / mappedValue >= MIN_SHARE).slice(0, MAX_BOXES);
  // A machine where every app is tiny would otherwise draw one anonymous box.
  const named = clearing.length > 0 ? clearing : sorted.slice(0, 1);
  const folded = sorted.slice(named.length);

  const namedValues = named.map((entry) => entry.value);
  const otherValue = folded.reduce((sum, entry) => sum + entry.value, 0);
  const rects = squarify(folded.length > 0 ? [...namedValues, otherValue] : namedValues);

  const boxes: TreemapBox[] = named.map((entry, i) => ({
    kind: "app",
    value: entry.value,
    share: entry.value / mappedValue,
    rect: rects[i],
    app: entry.app,
  }));
  if (folded.length > 0) {
    boxes.push({
      kind: "other",
      value: otherValue,
      share: otherValue / mappedValue,
      rect: rects[named.length],
      apps: folded.map((entry) => entry.app),
    });
  }

  return { boxes, mappedValue };
}
