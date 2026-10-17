import { describe, expect, it } from "vitest";
import { formatHourLabel, iconPathForKey, rankingByHour } from "./ranking";
import type { RankingMetric, RankingRow } from "./types";

const HOUR = 3_600_000;
const row = (hour: number, metric: RankingMetric, apps: [string, number][]): RankingRow => ({
  hourStartMs: hour * HOUR,
  metric,
  apps: apps.map(([key, value]) => ({ key, name: key, value })),
});

describe("rankingByHour", () => {
  const rows = [
    row(1, "memory", [["a", 100], ["b", 300], ["c", 200], ["d", 50]]),
    row(1, "cpu", [["x", 12], ["y", 5]]),
    row(2, "memory", [["a", 400]]),
    row(2, "cpu", [["y", 30], ["x", 10]]),
  ];

  it("takes one metric, newest hour first, and the three busiest of each hour", () => {
    const memory = rankingByHour(rows, "memory");
    expect(memory.map((h) => h.hourStartMs)).toEqual([2 * HOUR, 1 * HOUR]);
    expect(memory[1].apps.map((a) => a.key)).toEqual(["b", "c", "a"]);
    expect(memory[0].apps.map((a) => a.key)).toEqual(["a"]);
  });

  it("keeps the two metrics apart", () => {
    const cpu = rankingByHour(rows, "cpu");
    expect(cpu[0].apps.map((a) => a.key)).toEqual(["y", "x"]);
    expect(cpu[0].apps.every((a) => a.value <= 30)).toBe(true);
  });

  it("does not count an app that used nothing among the busiest, nor keep an empty hour", () => {
    const quiet = [row(3, "cpu", [["idle", 0]]), row(4, "cpu", [["busy", 2], ["idle", 0]])];
    const out = rankingByHour(quiet, "cpu");
    expect(out.map((h) => h.hourStartMs)).toEqual([4 * HOUR]);
    expect(out[0].apps.map((a) => a.key)).toEqual(["busy"]);
  });

  it("ignores readings that are not numbers", () => {
    const out = rankingByHour([row(1, "cpu", [["bad", Number.NaN], ["ok", 3]])], "cpu");
    expect(out[0].apps.map((a) => a.key)).toEqual(["ok"]);
  });

  it("merges two rows for the same hour and metric, keeping each app once", () => {
    const out = rankingByHour([row(1, "memory", [["a", 10], ["b", 5]]), row(1, "memory", [["a", 20]])], "memory");
    expect(out).toHaveLength(1);
    expect(out[0].apps).toEqual([
      { key: "a", name: "a", value: 20 },
      { key: "b", name: "b", value: 5 },
    ]);
  });

  it("has nothing for an empty record", () => {
    expect(rankingByHour([], "memory")).toEqual([]);
    expect(rankingByHour(rows, "memory", 0)).toEqual([]);
  });
});

describe("iconPathForKey", () => {
  it("returns the path of a key, without a service role", () => {
    expect(iconPathForKey("c:\\program files\\app\\app.exe")).toBe("c:\\program files\\app\\app.exe");
    expect(iconPathForKey("c:\\windows\\system32\\svchost.exe|netsvcs")).toBe("c:\\windows\\system32\\svchost.exe");
    expect(iconPathForKey("\\\\server\\share\\tool.exe")).toBe("\\\\server\\share\\tool.exe");
  });

  it("returns nothing for a key that is not a path", () => {
    expect(iconPathForKey("unreadable-processes")).toBeUndefined();
    expect(iconPathForKey("")).toBeUndefined();
  });
});

describe("formatHourLabel", () => {
  const now = Date.UTC(2026, 9, 10, 15, 30);

  it("is a clock time for an hour of today", () => {
    expect(formatHourLabel(Date.UTC(2026, 9, 10, 14), now, "en-GB", "UTC")).toBe("14:00");
  });

  it("names the weekday for an earlier day", () => {
    const label = formatHourLabel(Date.UTC(2026, 9, 9, 22), now, "en-GB", "UTC");
    expect(label).toContain("Fri");
    expect(label).toContain("22:00");
  });

  it("decides what is today in the time zone it is shown in", () => {
    // 23:00 UTC on the 9th is 08:00 on the 10th in Tokyo, the same day as 10:00 UTC on the 10th.
    const hour = Date.UTC(2026, 9, 9, 23);
    const earlier = Date.UTC(2026, 9, 10, 10);
    expect(formatHourLabel(hour, earlier, "en-GB", "Asia/Tokyo")).toBe("08:00");
    expect(formatHourLabel(hour, earlier, "en-GB", "UTC")).toContain("Fri");
  });
});
