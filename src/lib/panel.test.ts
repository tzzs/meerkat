import { describe, expect, it } from "vitest";
import { fitRows, panelContentHeight, panelNav, panelSeries, panelTabForKey, panelTiles, panelWindowSize } from "./panel";
import type { Snapshot } from "./types";

const machine = (hasBattery: boolean, hottest: number | null = null): Snapshot =>
  ({
    battery: { hasBattery },
    temperature: { hottest, sensors: [] },
  }) as unknown as Snapshot;

describe("panelNav", () => {
  it("ends with the battery on a laptop", () => {
    expect(panelNav(machine(true))).toEqual(["top", "cpu", "memory", "disk", "network", "gpu", "battery"]);
  });

  it("puts temperature in the power slot on a desktop that reports one", () => {
    expect(panelNav(machine(false, 51)).at(-1)).toBe("temperature");
  });

  it("drops the power slot when there is nothing to show in it", () => {
    expect(panelNav(machine(false))).toEqual(["top", "cpu", "memory", "disk", "network", "gpu"]);
  });
});

describe("panelTabForKey", () => {
  const nav = panelNav(machine(false));

  it("maps each digit to the tab at that position on screen", () => {
    expect(panelTabForKey("1", nav)).toBe("top");
    expect(panelTabForKey("6", nav)).toBe("gpu");
  });

  // "8" used to select temperature, a tab with no button in the strip.
  it("selects nothing past the last tab", () => {
    expect(panelTabForKey("7", nav)).toBeNull();
    expect(panelTabForKey("8", nav)).toBeNull();
  });

  it("ignores keys that are not a single digit from 1", () => {
    expect(panelTabForKey("0", nav)).toBeNull();
    expect(panelTabForKey("a", nav)).toBeNull();
    expect(panelTabForKey("F1", nav)).toBeNull();
  });
});

describe("panelTiles", () => {
  const tiles = [
    { id: "battery", visible: true },
    { id: "cpu", visible: true },
    { id: "sound", visible: true },
    { id: "gpu", visible: false },
    { id: "network", visible: true },
  ];

  it("keeps the saved order and drops what is hidden or cannot be drawn", () => {
    expect(panelTiles(tiles, machine(true))).toEqual(["battery", "cpu", "network"]);
  });

  it("leaves the battery out on a machine without one", () => {
    expect(panelTiles(tiles, machine(false))).toEqual(["cpu", "network"]);
  });
});

describe("panelSeries", () => {
  it("draws both directions of a rate that the tile prints both of", () => {
    const s = {
      disk: { historyRead: [1, 2], historyWrite: [10, 20] },
      network: { historyDownload: [5], historyUpload: [7] },
      cpu: { history: [3, 4] },
    } as unknown as Snapshot;
    expect(panelSeries("disk", s)).toEqual([11, 22]);
    expect(panelSeries("network", s)).toEqual([12]);
    expect(panelSeries("cpu", s)).toEqual([3, 4]);
  });
});

describe("panelContentHeight", () => {
  it("adds the natural heights of every section", () => {
    expect(panelContentHeight([40, 264, 204, 27])).toBe(535);
  });

  it("ignores a section that is not on screen", () => {
    expect(panelContentHeight([40, 100, 0, 27])).toBe(167);
  });

  it("does not let a bad measurement shrink the card", () => {
    expect(panelContentHeight([40, Number.NaN, -20, 27])).toBe(67);
  });
});

describe("panelWindowSize", () => {
  const box = { scrollWidth: 374, scrollHeight: 500, borderX: 2, borderY: 2, marginX: 24, marginY: 24 };

  it("holds the card, its border and its margins", () => {
    expect(panelWindowSize(box)).toEqual({ width: 400, height: 526 });
  });

  // A window sized from the card it contains must come out at the size it already is, or
  // every measurement would shrink it again.
  it("is stable when measured inside the window it produced", () => {
    const first = panelWindowSize(box);
    const again = panelWindowSize({ ...box, scrollWidth: first.width - 26 });
    expect(again).toEqual(first);
  });

  it("stays within the limits a tray panel can sensibly have", () => {
    expect(panelWindowSize({ ...box, scrollHeight: 2000 }).height).toBe(760);
    expect(panelWindowSize({ ...box, scrollHeight: 10 }).height).toBe(220);
    expect(panelWindowSize({ ...box, scrollWidth: 100 }).width).toBe(320);
  });

  it("rounds fractions up so the last pixel row is never cut", () => {
    expect(panelWindowSize({ ...box, scrollHeight: 500.2 }).height).toBe(527);
  });
});

describe("fitRows", () => {
  it("falls back to the minimum when the room fits nothing, never zero", () => {
    expect(fitRows(100, 120, 6, 3)).toBe(3);
  });

  it("stops at the list's real length when the room is larger than it", () => {
    expect(fitRows(1000, 20, 6, 3)).toBe(6);
  });

  it("treats an unmeasured room or row height as the minimum", () => {
    expect(fitRows(0, 20, 6, 3)).toBe(3);
    expect(fitRows(100, 0, 6, 3)).toBe(3);
  });

  it("floors a fractional row so a half-visible row is dropped whole", () => {
    expect(fitRows(100, 30, 6, 3)).toBe(3);
  });

  it("treats NaN, negatives and Infinity as the minimum", () => {
    expect(fitRows(Number.NaN, 20, 6, 3)).toBe(3);
    expect(fitRows(-5, 20, 6, 3)).toBe(3);
    expect(fitRows(Number.POSITIVE_INFINITY, 20, 6, 3)).toBe(3);
    expect(fitRows(100, Number.NEGATIVE_INFINITY, 6, 3)).toBe(3);
  });

  it("floors the minimum at the count, so one app is not padded to three", () => {
    expect(fitRows(0, 0, 1, 3)).toBe(1);
    expect(fitRows(1000, 20, 1, 3)).toBe(1);
  });
});
