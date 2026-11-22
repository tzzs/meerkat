import { describe, expect, it } from "vitest";
import { cpuAverages } from "./cpu-average";
import type { AppInfo } from "./types";

const app = (key: string, cpuPercent: number): AppInfo => ({ key, cpuPercent }) as AppInfo;

describe("cpuAverages", () => {
  it("starts at the first reading", () => {
    const samples = new Map<string, number[]>();
    expect(cpuAverages(samples, [app("a", 7), app("b", 3)])).toEqual(
      new Map([
        ["a", 7],
        ["b", 3],
      ]),
    );
  });

  it("means every reading in the window", () => {
    const samples = new Map<string, number[]>();
    for (const value of [2, 4, 9]) cpuAverages(samples, [app("a", value)]);
    expect(cpuAverages(samples, [app("a", 15)]).get("a")).toBe(7.5);
  });

  it("drops the oldest reading once the window is full", () => {
    const samples = new Map<string, number[]>();
    for (const value of [1, 2, 3, 4, 5, 6, 7]) cpuAverages(samples, [app("a", value)]);
    expect(samples.get("a")).toEqual([3, 4, 5, 6, 7]);
    expect(cpuAverages(samples, [app("a", 8)]).get("a")).toBe(6);
  });

  it("forgets an app that is gone", () => {
    const samples = new Map<string, number[]>();
    cpuAverages(samples, [app("a", 5), app("b", 6)]);
    const averages = cpuAverages(samples, [app("a", 5)]);
    expect([...averages.keys()]).toEqual(["a"]);
    expect(samples.has("b")).toBe(false);
  });

  it("leaves the caller's app list alone", () => {
    const apps = [app("a", 30), app("b", 10)];
    const before = apps.map((entry) => ({ ...entry }));
    cpuAverages(new Map(), apps);
    expect(apps).toEqual(before);
    expect(apps.map((entry) => entry.cpuPercent)).toEqual([30, 10]);
  });
});
