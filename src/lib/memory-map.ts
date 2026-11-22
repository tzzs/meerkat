import { squarify, type Rect } from "./storage";
import type { AppInfo } from "./types";

/** Enough boxes to name the programs people recognise without the tail turning into slivers. */
export const MAX_BOXES = 20;
/** Below this share of the measured bytes an app is folded into the "other" box. */
export const MIN_SHARE = 0.005;

export type MemoryBox =
  | { kind: "app"; bytes: number; share: number; rect: Rect; app: AppInfo }
  | { kind: "other"; bytes: number; share: number; count: number; rect: Rect };

export interface MemoryMapData {
  boxes: MemoryBox[];
  mappedBytes: number;
}

/**
 * The apps that hold RAM, as boxes sized by working set. The biggest few are named; everything
 * below `MIN_SHARE`, and anything past `MAX_BOXES`, is folded into one box so the map still
 * covers every byte Meerkat measured.
 */
export function memoryMap(apps: readonly AppInfo[]): MemoryMapData {
  const measured = apps.filter((app) => Number.isFinite(app.memoryBytes) && app.memoryBytes > 0);
  const mappedBytes = measured.reduce((sum, app) => sum + app.memoryBytes, 0);
  if (!(mappedBytes > 0)) return { boxes: [], mappedBytes: 0 };

  // `sort` is stable, so equal working sets keep the order the sampler sent them in.
  const sorted = [...measured].sort((a, b) => b.memoryBytes - a.memoryBytes);
  // The list is sorted, so the apps that clear the floor are a prefix: the fold can start
  // where the named ones end.
  const clearing = sorted.filter((app) => app.memoryBytes / mappedBytes >= MIN_SHARE).slice(0, MAX_BOXES);
  // A machine where every app is tiny would otherwise draw one anonymous box.
  const named = clearing.length > 0 ? clearing : sorted.slice(0, 1);
  const folded = sorted.slice(named.length);

  const namedBytes = named.map((app) => app.memoryBytes);
  const otherBytes = folded.reduce((sum, app) => sum + app.memoryBytes, 0);
  const rects = squarify(folded.length > 0 ? [...namedBytes, otherBytes] : namedBytes);

  const boxes: MemoryBox[] = named.map((app, i) => ({
    kind: "app",
    bytes: app.memoryBytes,
    share: app.memoryBytes / mappedBytes,
    rect: rects[i],
    app,
  }));
  if (folded.length > 0) {
    boxes.push({
      kind: "other",
      bytes: otherBytes,
      share: otherBytes / mappedBytes,
      count: folded.length,
      rect: rects[named.length],
    });
  }

  return { boxes, mappedBytes };
}
