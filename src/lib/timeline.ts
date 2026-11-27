import { GAP_BUCKETS, bucketMilliseconds } from "./series";
import type { HistoryPoint } from "./types";

/** A stretch of the time axis, in milliseconds since the epoch. */
export interface Span {
  fromMs: number;
  toMs: number;
}

/**
 * The parts of `[fromMs, toMs]` with nothing recorded: before the first bucket, after the last,
 * and every stretch between two buckets too far apart to be one continuous run.
 *
 * The chart used to leave these stretches blank and say nothing about them, so a machine Meerkat
 * had been watching for half an hour looked like a chart with one vertical bar at its right edge
 * rather than like thirty minutes of record inside a day. Each bucket covers `bucketSeconds`
 * from its own time; a bucket is only assumed to run to the next one when the next one is less
 * than `GAP_BUCKETS` buckets away.
 */
export function blankSpans(
  points: readonly HistoryPoint[],
  fromMs: number,
  toMs: number,
  bucketSeconds: number,
): Span[] {
  const bucket = bucketMilliseconds(bucketSeconds);
  if (!(toMs > fromMs)) return [];
  const inside = points
    .filter((point) => point.atMs >= fromMs && point.atMs <= toMs && Number.isFinite(point.value))
    .sort((a, b) => a.atMs - b.atMs);

  const blanks: Span[] = [];
  let cursor = fromMs;
  for (const point of inside) {
    if (point.atMs - cursor > (GAP_BUCKETS - 1) * bucket) blanks.push({ fromMs: cursor, toMs: point.atMs });
    cursor = Math.max(cursor, point.atMs + bucket);
  }
  if (toMs - cursor > 0) blanks.push({ fromMs: cursor, toMs });
  return blanks;
}

/**
 * The stretch the records actually reach, or null when there are none: the first bucket's start
 * to the last bucket's end, inside the window. A range that is mostly empty draws its data as a
 * sliver against one edge; this is what "only show what was recorded" would show instead.
 */
export function recordedSpan(
  points: readonly HistoryPoint[],
  fromMs: number,
  toMs: number,
  bucketSeconds: number,
): Span | null {
  const bucket = bucketMilliseconds(bucketSeconds);
  let first = Infinity;
  let last = -Infinity;
  for (const point of points) {
    if (!Number.isFinite(point.value) || point.atMs < fromMs || point.atMs > toMs) continue;
    first = Math.min(first, point.atMs);
    last = Math.max(last, point.atMs + bucket);
  }
  if (first === Infinity) return null;
  return { fromMs: first, toMs: Math.min(last, toMs) };
}

/**
 * The start of the bucket containing `atMs`. Backend buckets are epoch-aligned and cover the
 * whole interval from their timestamp to the next one, so a click at 10:08 inside a ten-minute
 * bucket belongs to 10:00, not 10:10. A click into a stretch with no record still lands on the
 * local grid rather than teleporting to a recorded bucket at the other end of the range.
 */
export function snapToBucket(atMs: number, bucketSeconds: number): number {
  return Math.floor(atMs / bucketMilliseconds(bucketSeconds)) * bucketMilliseconds(bucketSeconds);
}
