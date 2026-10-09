import { describe, expect, it } from "vitest";
import { headline, isMetricKind, powerTabMode, UNREADABLE_GROUP, type Snapshot } from "./types";

/** The power tab only ever reads these two fields, so a test snapshot carries no more. */
const snap = (over: { hasBattery?: boolean; hottest?: number | null; sensors?: string[] }): Snapshot =>
  ({
    battery: { hasBattery: over.hasBattery ?? false },
    temperature: { hottest: over.hottest ?? null, sensors: over.sensors ?? [] },
  }) as unknown as Snapshot;

describe("powerTabMode", () => {
  it("assumes a battery until the machine says otherwise", () => {
    expect(powerTabMode(null)).toBe("battery");
  });

  it("shows the battery tab on a laptop", () => {
    expect(powerTabMode(snap({ hasBattery: true }))).toBe("battery");
  });

  it("renames the same tab to temperature when there is a temperature but no battery", () => {
    expect(powerTabMode(snap({ hasBattery: false, hottest: 45 }))).toBe("thermal");
    expect(powerTabMode(snap({ hasBattery: false, hottest: null, sensors: ["acpitz"] }))).toBe("thermal");
  });

  it("hides the tab on a desktop with nothing readable, rather than opening on an empty page", () => {
    expect(powerTabMode(snap({ hasBattery: false, hottest: null, sensors: [] }))).toBe("hidden");
  });

  it("prefers the battery when a laptop also has temperatures", () => {
    expect(powerTabMode(snap({ hasBattery: true, hottest: 60, sensors: ["x"] }))).toBe("battery");
  });
});

describe("isMetricKind", () => {
  it("accepts every metric the panel can draw", () => {
    for (const id of ["cpu", "memory", "disk", "network", "gpu", "battery", "temperature"]) {
      expect(isMetricKind(id)).toBe(true);
    }
  });

  // The panel's default tiles used to be built from the tab list, which put "sound" among the
  // metrics; drawing a tile with no series behind it threw during render and left the whole
  // tray panel blank.
  it("rejects tab ids, which share six of their names with metrics", () => {
    expect(isMetricKind("sound")).toBe(false);
    expect(isMetricKind("overview")).toBe(false);
    expect(isMetricKind("")).toBe(false);
    expect(isMetricKind("CPU")).toBe(false);
  });
});

describe("headline", () => {
  it("reports memory as a percentage of what is installed", () => {
    const s = { memory: { totalBytes: 200, inUseBytes: 50 } } as unknown as Snapshot;
    expect(headline("memory", s)).toBe(25);
  });

  it("says zero rather than NaN when the machine has reported no total yet", () => {
    const s = { memory: { totalBytes: 0, inUseBytes: 12 } } as unknown as Snapshot;
    expect(headline("memory", s)).toBe(0);
  });
});

describe("UNREADABLE_GROUP", () => {
  it("is not a metric, so it can never be drawn as a panel tile", () => {
    expect(isMetricKind(UNREADABLE_GROUP)).toBe(false);
  });
});
