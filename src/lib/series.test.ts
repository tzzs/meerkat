import { describe, expect, it } from "vitest";
import { defaultUnits, type UnitPrefs } from "./format";
import {
  buildSeries,
  formatAxisTime,
  formatHistoryValue,
  historyCeiling,
  historyUnit,
  seriesPaths,
} from "./series";

const MINUTE = 60_000;
const HOUR = 3_600_000;
const NBSP = " ";
const point = (atMs: number, value: number, secondary: number | null = null) => ({ atMs, value, secondary });
const units = (over: Partial<UnitPrefs> = {}): UnitPrefs => ({ ...defaultUnits, ...over });

describe("buildSeries", () => {
  it("places points by time, not by index", () => {
    // Half-hour buckets, so these three are neighbours rather than separated by missing data.
    const built = buildSeries([point(0, 1), point(HOUR / 2, 2), point(HOUR, 3)], 0, HOUR, 1800);
    expect(built.segments).toHaveLength(1);
    expect(built.segments[0].map((p) => p.x)).toEqual([0, 0.5, 1]);
  });

  it("spaces the same points differently when time between them differs", () => {
    // By index these would be 0, 0.5, 1; by time the third is far from the second.
    const built = buildSeries([point(0, 1), point(MINUTE, 1), point(10 * MINUTE, 1)], 0, 10 * MINUTE, 600);
    expect(built.segments[0].map((p) => p.x)).toEqual([0, 0.1, 1]);
  });

  // This machine's twelve hours of history had a 195-minute hole that used to be drawn as a line.
  it("breaks the line where data is missing", () => {
    const points = [point(0, 1), point(MINUTE, 1), point(MINUTE + 195 * MINUTE, 1), point(197 * MINUTE, 1)];
    const built = buildSeries(points, 0, 12 * HOUR, 60);
    expect(built.segments.map((run) => run.length)).toEqual([2, 2]);
  });

  it("scales the gap that breaks a line with the bucket", () => {
    // Two missing hourly buckets (a three-hour step) is a gap; one missing bucket is not.
    expect(buildSeries([point(0, 1), point(2 * HOUR, 1)], 0, 24 * HOUR, 3600).segments).toHaveLength(1);
    expect(buildSeries([point(0, 1), point(3 * HOUR, 1)], 0, 24 * HOUR, 3600).segments).toHaveLength(2);
  });

  it("does not break for a gap within two and a half buckets", () => {
    const built = buildSeries([point(0, 1), point(150_000, 1)], 0, HOUR, 60);
    expect(built.segments).toHaveLength(1);
    expect(buildSeries([point(0, 1), point(150_001, 1)], 0, HOUR, 60).segments).toHaveLength(2);
  });

  it("drops what falls outside the window", () => {
    const built = buildSeries([point(-1, 9), point(10, 1), point(HOUR + 1, 9)], 0, HOUR, 60);
    expect(built.segments.flat()).toHaveLength(1);
    expect(built.stats).toEqual({ min: 1, avg: 1, peak: 1 });
  });

  it("clips a window that starts late, so the points before it do not count", () => {
    const built = buildSeries([point(0, 100), point(HOUR, 10), point(HOUR + MINUTE, 20)], HOUR, 2 * HOUR, 60);
    expect(built.stats).toEqual({ min: 10, avg: 15, peak: 20 });
    expect(built.segments[0].map((p) => p.x)).toEqual([0, MINUTE / HOUR]);
  });

  it("has nothing to draw and no statistics without data", () => {
    expect(buildSeries([], 0, HOUR, 60)).toEqual({ segments: [], bucketFraction: MINUTE / HOUR, stats: null });
    expect(buildSeries([point(5, 1)], HOUR, HOUR, 60).segments).toEqual([]);
    expect(buildSeries([point(5, 1)], HOUR, 0, 60).stats).toBeNull();
  });

  it("keeps a single point as a run of its own", () => {
    const built = buildSeries([point(HOUR / 4, 7)], 0, HOUR, 60);
    expect(built.segments).toEqual([[{ x: 0.25, mean: 7, max: 7 }]]);
    expect(built.stats).toEqual({ min: 7, avg: 7, peak: 7 });
  });

  it("takes the peak from the bucket maxima and the low and average from the means", () => {
    const built = buildSeries([point(0, 10, 40), point(60_000, 20, 25), point(120_000, 30, null)], 0, HOUR, 60);
    expect(built.stats).toEqual({ min: 10, avg: 20, peak: 40 });
    expect(built.segments[0].map((p) => p.max)).toEqual([40, 25, 30]);
  });

  it("never lets a maximum sit below its own mean", () => {
    const built = buildSeries([point(0, 10, 4)], 0, HOUR, 60);
    expect(built.segments[0][0].max).toBe(10);
  });

  it("sorts points that arrive out of order", () => {
    const built = buildSeries([point(HOUR, 3), point(0, 1)], 0, HOUR, 3600);
    expect(built.segments[0].map((p) => p.mean)).toEqual([1, 3]);
  });

  it("ignores a reading that is not a number rather than drawing it", () => {
    const built = buildSeries([point(0, Number.NaN), point(MINUTE, 5)], 0, HOUR, 60);
    expect(built.segments.flat()).toHaveLength(1);
    expect(built.stats?.avg).toBe(5);
  });

  it("reads a missing bucket size as the finest the backend keeps", () => {
    const built = buildSeries([point(0, 1), point(4 * MINUTE, 1)], 0, HOUR, Number.NaN);
    expect(built.segments).toHaveLength(2);
    expect(built.bucketFraction).toBe(MINUTE / HOUR);
  });
});

describe("seriesPaths", () => {
  it("draws one subpath per run", () => {
    const built = buildSeries([point(0, 50), point(MINUTE, 50), point(30 * MINUTE, 50), point(31 * MINUTE, 50)], 0, HOUR, 60);
    const paths = seriesPaths(built, 240, 100, 100);
    expect(paths.line.match(/M/g)).toHaveLength(2);
    expect(paths.area.match(/Z/g)).toHaveLength(2);
    expect(paths.envelope.match(/Z/g)).toHaveLength(2);
  });

  it("gives a lone point a visible width", () => {
    const built = buildSeries([point(HOUR / 2, 50)], 0, HOUR, 60);
    expect(seriesPaths(built, 240, 100, 100).line).toBe("M120.00,50.00 L124.00,50.00");
  });

  it("keeps a lone point at the right edge inside the box", () => {
    const built = buildSeries([point(HOUR, 50)], 0, HOUR, 60);
    expect(seriesPaths(built, 240, 100, 100).line).toBe("M236.00,50.00 L240.00,50.00");
  });

  it("puts the maximum above the mean", () => {
    const built = buildSeries([point(0, 20, 80), point(MINUTE, 20, 80)], 0, HOUR, 60);
    const paths = seriesPaths(built, 240, 100, 100);
    expect(paths.line).toBe("M0.00,80.00 L4.00,80.00");
    expect(paths.envelope).toContain("0.00,20.00");
  });

  it("keeps everything inside the box", () => {
    const built = buildSeries([point(0, 500, 900), point(60_000, -3)], 0, HOUR, 60);
    const numbers = seriesPaths(built, 240, 100, 100).envelope.match(/-?\d+(\.\d+)?/g)!.map(Number);
    expect(Math.min(...numbers)).toBeGreaterThanOrEqual(0);
    expect(Math.max(...numbers)).toBeLessThanOrEqual(240);
  });

  it("draws nothing for an empty series", () => {
    expect(seriesPaths(buildSeries([], 0, HOUR, 60), 240, 100, 100)).toEqual({ line: "", area: "", envelope: "" });
  });
});

describe("historyUnit", () => {
  it("knows every metric the backend records", () => {
    const expected: Record<string, string> = {
      cpu: "percent",
      memory: "percent",
      gpu: "percent",
      battery: "percent",
      memoryUsed: "bytes",
      memoryTotal: "bytes",
      available: "bytes",
      committed: "bytes",
      diskRead: "storageRate",
      diskWrite: "storageRate",
      download: "networkRate",
      upload: "networkRate",
      temp: "celsius",
    };
    for (const [metric, unit] of Object.entries(expected)) expect(historyUnit(metric)).toBe(unit);
  });

  it("does not guess at an unknown metric, including one that is an object property", () => {
    expect(historyUnit("fan")).toBeNull();
    expect(historyUnit("constructor")).toBeNull();
  });
});

describe("historyCeiling", () => {
  it("plots a percentage against 100 whatever the window held", () => {
    expect(historyCeiling("percent", 3)).toBe(100);
    expect(historyCeiling("percent", 0)).toBe(100);
  });

  it("does not clip a value that passes the nominal top", () => {
    expect(historyCeiling("percent", 140)).toBe(140);
    expect(historyCeiling("celsius", 120)).toBe(120);
  });

  it("fits a rate to its own peak with a little headroom", () => {
    expect(historyCeiling("storageRate", 1000)).toBeCloseTo(1050);
    expect(historyCeiling("networkRate", 0)).toBe(1);
    expect(historyCeiling(null, Number.NaN)).toBe(1);
  });
});

describe("formatHistoryValue", () => {
  it("writes each metric in the user's units", () => {
    expect(formatHistoryValue("cpu", 42, units())).toBe("42%");
    expect(formatHistoryValue("memoryUsed", 21_300_000_000, units())).toBe(`21.3${NBSP}GB`);
    expect(formatHistoryValue("diskWrite", 10_240, units())).toBe(`10.2${NBSP}KB/s`);
    expect(formatHistoryValue("download", 1_000_000, units({ network: "bits" }))).toBe(`8.00${NBSP}Mbit/s`);
    expect(formatHistoryValue("temp", 37, units({ temperature: "fahrenheit" }))).toBe("99°F");
  });

  it("keeps a decimal for a quiet machine's single digits", () => {
    expect(formatHistoryValue("cpu", 2.34, units())).toBe("2.3%");
    expect(formatHistoryValue("cpu", 0, units())).toBe("0%");
  });
});

describe("formatAxisTime", () => {
  const at = Date.UTC(2026, 9, 10, 14, 5);

  it("uses the clock within half a day", () => {
    expect(formatAxisTime(at, 3600, "en-GB", "UTC")).toBe("14:05");
    expect(formatAxisTime(at, 12 * 3600, "en-GB", "UTC")).toBe("14:05");
  });

  it("names the weekday for a day and the date beyond it", () => {
    expect(formatAxisTime(at, 24 * 3600, "en-GB", "UTC")).toContain("Sat");
    expect(formatAxisTime(at, 24 * 3600, "en-GB", "UTC")).toContain("14:05");
    expect(formatAxisTime(at, 7 * 86400, "en-GB", "UTC")).toMatch(/Sat/);
    expect(formatAxisTime(at, 7 * 86400, "en-GB", "UTC")).not.toContain("14:05");
  });
});
