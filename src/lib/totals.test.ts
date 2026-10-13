import { describe, expect, it } from "vitest";
import { capacityTrend, sumDays } from "./totals";
import type { DailyTotal } from "./types";

const day = (n: number, bytes: number): DailyTotal => ({
  dayStartMs: n * 86_400_000,
  readBytes: bytes,
  writeBytes: bytes * 2,
  downloadBytes: bytes * 3,
  uploadBytes: bytes * 4,
});

describe("sumDays", () => {
  const rows = [day(1, 10), day(2, 20), day(3, 30)];

  it("adds up the last few days", () => {
    expect(sumDays(rows, 2).read).toBe(50);
    expect(sumDays(rows, 30).upload).toBe(240);
  });

  it("uses the live count for today, which is newer than the stored one", () => {
    expect(sumDays(rows, 1, { download: 500 }).download).toBe(500);
    expect(sumDays(rows, 2, { download: 500 }).download).toBe(60 + 500);
    // A figure not given live stays as stored.
    expect(sumDays(rows, 2, { download: 500 }).read).toBe(50);
  });

  it("falls back to the live count before anything is stored", () => {
    expect(sumDays([], 7, { read: 3 })).toEqual({ read: 3, write: 0, download: 0, upload: 0 });
  });
});

describe("capacityTrend", () => {
  it("compares the first week with the last", () => {
    const trend = capacityTrend([
      { atMs: 0, designWh: 50, fullChargeWh: 49, cycleCount: null },
      { atMs: 7 * 86_400_000 * 52, designWh: 50, fullChargeWh: 41, cycleCount: 300 },
    ]);
    expect(trend).toEqual({ from: 98, to: 82, weeks: 52 });
  });

  it("needs two weeks", () => {
    expect(capacityTrend([{ atMs: 0, designWh: 50, fullChargeWh: 49, cycleCount: null }])).toBeNull();
  });
});
