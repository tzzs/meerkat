import { describe, expect, it } from "vitest";
import { holdCeiling, niceCeiling, SPARK_CEILING, sparkPoints, sumSeries, windowPeak } from "./spark";

const xs = (points: string) => points.split(" ").map((p) => Number(p.split(",")[0]));
const ys = (points: string) => points.split(" ").map((p) => Number(p.split(",")[1]));
const ramp = (n: number) => Array.from({ length: n }, (_, i) => i % 100);

describe("sparkPoints", () => {
  it("draws nothing for no samples rather than a flat line at a zero nobody measured", () => {
    expect(sparkPoints([], 100, 22, 100)).toBe("");
  });

  it("holds a single sample as a level across the whole box", () => {
    expect(sparkPoints([50], 100, 22, 100)).toBe("0,11.0 100,11.0");
  });

  it("puts two samples at the two edges", () => {
    expect(xs(sparkPoints([0, 100], 100, 22, 100))).toEqual([0, 100]);
    expect(ys(sparkPoints([0, 100], 100, 22, 100))).toEqual([22, 0]);
  });

  it("fills the full width with a minute of samples", () => {
    const points = sparkPoints(ramp(60), 100, 22, 100);
    expect(xs(points)).toHaveLength(60);
    expect(xs(points)[0]).toBe(0);
    expect(xs(points).at(-1)).toBe(100);
  });

  // The regression: the step came from all 240 samples and only the last 60 were drawn, so
  // every line ended at x = 24.7 of 100 from the fourth minute on.
  it("cuts the window before working out the step, so a long history still spans the box", () => {
    const points = sparkPoints(ramp(240), 100, 22, 100);
    expect(xs(points)).toHaveLength(60);
    expect(xs(points).at(-1)).toBe(100);
    expect(Math.max(...xs(points))).toBe(100);
  });

  it("draws the newest samples, not the oldest", () => {
    const values = [...Array(200).fill(0), ...Array(60).fill(100)];
    expect(new Set(ys(sparkPoints(values, 100, 22, 100)))).toEqual(new Set([0]));
  });

  it("keeps a reading inside the box when it passes the ceiling or is not a number", () => {
    expect(ys(sparkPoints([150, -5, Number.NaN], 100, 20, 100))).toEqual([0, 20, 20]);
  });

  it("treats a ceiling of zero as a unit scale rather than dividing by it", () => {
    expect(ys(sparkPoints([0, 1], 100, 20, 0))).toEqual([20, 0]);
  });
});

describe("SPARK_CEILING", () => {
  it("fixes percentages at 100 so an idle machine does not fill its tile", () => {
    expect(SPARK_CEILING.cpu).toBe(100);
    expect(SPARK_CEILING.memory).toBe(100);
    expect(SPARK_CEILING.gpu).toBe(100);
    expect(SPARK_CEILING.battery).toBe(100);
  });

  it("leaves rates to scale themselves", () => {
    expect(SPARK_CEILING.disk).toBeUndefined();
    expect(SPARK_CEILING.network).toBeUndefined();
  });
});

describe("niceCeiling", () => {
  it("rounds up to a 1, 2 or 5 step", () => {
    expect(niceCeiling(3)).toBe(5);
    expect(niceCeiling(12)).toBe(20);
    expect(niceCeiling(100)).toBe(100);
    expect(niceCeiling(4_200_000)).toBe(5_000_000);
  });

  it("never returns zero, which would be divided by", () => {
    expect(niceCeiling(0)).toBe(1);
    expect(niceCeiling(-3)).toBe(1);
    expect(niceCeiling(Number.NaN)).toBe(1);
  });
});

describe("holdCeiling", () => {
  it("rises at once for a new peak", () => {
    expect(holdCeiling({ ceiling: 10, below: 3 }, 45, 6)).toEqual({ ceiling: 50, below: 0 });
  });

  it("only falls after enough readings in a row fit under a lower one", () => {
    let state = { ceiling: 50, below: 0 };
    for (let i = 0; i < 5; i++) state = holdCeiling(state, 4, 6);
    expect(state.ceiling).toBe(50);
    state = holdCeiling(state, 4, 6);
    expect(state).toEqual({ ceiling: 5, below: 0 });
  });

  it("starts the count again when a reading needs the ceiling it has", () => {
    let state = holdCeiling({ ceiling: 50, below: 0 }, 4, 6);
    state = holdCeiling(state, 48, 6);
    expect(state).toEqual({ ceiling: 50, below: 0 });
  });
});

describe("sumSeries", () => {
  it("adds sample by sample, aligned on the newest end", () => {
    expect(sumSeries([1, 2, 3], [10, 20, 30])).toEqual([11, 22, 33]);
    expect(sumSeries([1, 2, 3], [30])).toEqual([1, 2, 33]);
    expect(sumSeries([], [])).toEqual([]);
  });
});

describe("windowPeak", () => {
  it("only looks at the samples the sparkline draws", () => {
    expect(windowPeak([900, ...Array(60).fill(3)])).toBe(3);
    expect(windowPeak([])).toBe(0);
  });
});
