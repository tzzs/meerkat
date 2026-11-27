import { describe, expect, it } from "vitest";
import { blankSpans, isSameClick, recordedSpan, snapToBucket } from "./timeline";

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const point = (atMs: number, value = 1) => ({ atMs, value, secondary: null });

describe("blankSpans", () => {
  // The report that started this: a day-wide range, Meerkat running for the last half hour, so
  // almost the whole chart is a gap and nothing said so.
  it("reports the leading, trailing and inner stretches with no record", () => {
    const points = [point(20 * HOUR), point(20 * HOUR + 10 * MINUTE), point(23 * HOUR)];
    expect(blankSpans(points, 0, DAY, 600)).toEqual([
      { fromMs: 0, toMs: 20 * HOUR },
      // 20:10's bucket runs to 20:20; the next record is 23:00.
      { fromMs: 20 * HOUR + 20 * MINUTE, toMs: 23 * HOUR },
      { fromMs: 23 * HOUR + 10 * MINUTE, toMs: DAY },
    ]);
  });

  it("counts a run of buckets that fills the window as no gap at all", () => {
    const points = [point(0), point(10 * MINUTE), point(20 * MINUTE)];
    expect(blankSpans(points, 0, 30 * MINUTE, 600)).toEqual([]);
  });

  it("does not invent a gap for one missing bucket", () => {
    // Buckets at 0, 20 and 30 minutes: one bucket (10 min) is missing but the run is unbroken.
    const points = [point(0), point(20 * MINUTE), point(30 * MINUTE)];
    expect(blankSpans(points, 0, 40 * MINUTE, 600)).toEqual([]);
  });

  it("says nothing when the window holds no data at all", () => {
    expect(blankSpans([], 0, HOUR, 60)).toEqual([{ fromMs: 0, toMs: HOUR }]);
  });

  it("ignores points outside the window and an empty window", () => {
    expect(blankSpans([point(-HOUR), point(2 * HOUR)], 0, HOUR, 60)).toEqual([{ fromMs: 0, toMs: HOUR }]);
    expect(blankSpans([point(0)], HOUR, HOUR, 60)).toEqual([]);
  });
});

describe("recordedSpan", () => {
  it("is the first bucket's start to the last bucket's end", () => {
    const points = [point(5 * HOUR), point(6 * HOUR)];
    expect(recordedSpan(points, 0, DAY, 600)).toEqual({ fromMs: 5 * HOUR, toMs: 6 * HOUR + 10 * MINUTE });
  });

  it("is null with no record, and never reaches past the window", () => {
    expect(recordedSpan([], 0, DAY, 600)).toBeNull();
    expect(recordedSpan([point(DAY)], 0, DAY, 600)).toEqual({ fromMs: DAY, toMs: DAY });
  });
});

describe("snapToBucket", () => {
  const points = [point(10 * MINUTE), point(20 * MINUTE)];

  it("picks the bucket a click inside it landed in", () => {
    expect(snapToBucket(10 * MINUTE + MINUTE, points, 600)).toBe(10 * MINUTE);
  });

  // Clicking a blank stretch must not jump the marker to the data at the other end of the day.
  it("rounds to the grid when no bucket is near", () => {
    expect(snapToBucket(5 * HOUR + 4 * MINUTE, points, 600)).toBe(5 * HOUR);
    expect(snapToBucket(5 * HOUR + 7 * MINUTE, points, 600)).toBe(5 * HOUR + 10 * MINUTE);
    expect(snapToBucket(5 * HOUR + 7 * MINUTE, [], 600)).toBe(5 * HOUR + 10 * MINUTE);
  });
});

describe("isSameClick", () => {
  it("counts a click within a few pixels of the marker as the same moment", () => {
    const msPerPixel = 66_000;
    expect(isSameClick(10 * MINUTE, 10 * MINUTE + 3 * msPerPixel, msPerPixel)).toBe(true);
    expect(isSameClick(10 * MINUTE, 10 * MINUTE + 30 * msPerPixel, msPerPixel)).toBe(false);
    // A coarse range makes every pixel worth minutes, and the tolerance follows the pixels.
    expect(isSameClick(0, 4 * MINUTE, 60_000)).toBe(true);
  });
});
