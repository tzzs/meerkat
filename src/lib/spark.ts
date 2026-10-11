import type { MetricKind } from "./types";

/** A sparkline is the last minute: sixty samples at the sampler's one per second. */
export const SPARK_WINDOW = 60;

/**
 * The fixed top of the scale for metrics that have one. A percentage cannot pass 100, so a
 * ceiling fitted to the data would only make an idle machine look busy: 3% drawn against a
 * ceiling of 5 fills the tile. Temperature is held to the same 110°C the sensor meters use.
 * Rates have no natural maximum and are absent here; they auto-scale with a held ceiling.
 */
export const SPARK_CEILING: Partial<Record<MetricKind, number>> = {
  cpu: 100,
  memory: 100,
  gpu: 100,
  battery: 100,
  temperature: 110,
};

/** Rounds up to the next 1/2/5 step, so an axis ceiling is a number a person can read. */
export function niceCeiling(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1;
  const base = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 5, 10].find((s) => value <= s * base) ?? 10;
  return step * base;
}

export interface HeldCeiling {
  ceiling: number;
  /** How many readings in a row have wanted a lower ceiling than the one held. */
  below: number;
}

/**
 * One reading's worth of a ceiling that rises at once and falls slowly.
 *
 * Rescaling to the visible maximum on every sample makes the whole curve change height each
 * second even when the data did not move, which reads as the graph bouncing. A new peak still
 * raises the ceiling immediately, because understating a spike would be a lie about the
 * measurement; it only comes back down after `hold` readings in a row that fit under less.
 */
export function holdCeiling(state: HeldCeiling, observed: number, hold: number): HeldCeiling {
  const next = niceCeiling(observed);
  if (next > state.ceiling) return { ceiling: next, below: 0 };
  if (next === state.ceiling) return state.below === 0 ? state : { ceiling: next, below: 0 };
  const below = state.below + 1;
  return below >= hold ? { ceiling: next, below: 0 } : { ceiling: state.ceiling, below };
}

/**
 * The points of a sparkline polyline, `x,y x,y …`, in a `width` × `height` box.
 *
 * The window is cut before the step is worked out. It used to be the other way round — the
 * step came from the full 240-sample history and only the last 60 were drawn — so from the
 * fourth minute on every line stopped a quarter of the way across its tile.
 *
 * No samples draws nothing rather than a flat line at zero, which would be a reading nobody
 * took; a single sample is that level held across the box.
 */
export function sparkPoints(
  values: readonly number[],
  width: number,
  height: number,
  ceiling: number,
  window = SPARK_WINDOW,
): string {
  const shown = values.length > window ? values.slice(-window) : values;
  if (shown.length === 0) return "";
  const top = Number.isFinite(ceiling) && ceiling > 0 ? ceiling : 1;
  const y = (v: number) => {
    const ratio = Math.min(1, Math.max(0, (Number.isFinite(v) ? v : 0) / top));
    return (height - ratio * height).toFixed(1);
  };
  if (shown.length === 1) {
    const level = y(shown[0]);
    return `0,${level} ${width},${level}`;
  }
  const step = width / (shown.length - 1);
  return shown.map((v, i) => `${(i * step).toFixed(1)},${y(v)}`).join(" ");
}

/**
 * Two histories added sample by sample, aligned on their newest end: while the sampler warms
 * up the two can differ in length, and the last sample of each is the same second.
 */
export function sumSeries(a: readonly number[], b: readonly number[]): number[] {
  const n = Math.max(a.length, b.length);
  const out = new Array<number>(n);
  for (let i = 0; i < n; i++) {
    const ai = a.length - n + i;
    const bi = b.length - n + i;
    out[i] = (ai >= 0 ? a[ai] : 0) + (bi >= 0 ? b[bi] : 0);
  }
  return out;
}

/** The largest value a sparkline will actually draw, which is what an auto scale fits to. */
export function windowPeak(values: readonly number[], window = SPARK_WINDOW): number {
  let peak = 0;
  for (let i = Math.max(0, values.length - window); i < values.length; i++) {
    const v = values[i];
    if (Number.isFinite(v) && v > peak) peak = v;
  }
  return peak;
}
