import { describe, expect, it } from "vitest";
import { MAX_BOXES, memoryMap, MIN_SHARE, type MemoryBox } from "./memory-map";
import type { AppInfo } from "./types";

const app = (key: string, memoryBytes: number): AppInfo => ({ key, memoryBytes }) as AppInfo;

const area = (rect: { w: number; h: number }) => rect.w * rect.h;
const appBoxes = (boxes: readonly MemoryBox[]) => boxes.filter((box) => box.kind === "app");

describe("memoryMap", () => {
  it("draws one box per app, proportional to its share", () => {
    const map = memoryMap([app("a", 40), app("b", 30), app("c", 20), app("d", 10)]);
    expect(map.boxes).toHaveLength(4);
    expect(map.boxes.every((box) => box.kind === "app")).toBe(true);
    expect(map.boxes.reduce((sum, box) => sum + area(box.rect), 0)).toBeCloseTo(1, 12);
    for (const box of map.boxes) {
      expect(area(box.rect)).toBeCloseTo(box.share, 12);
      expect(box.rect.x).toBeGreaterThanOrEqual(0);
      expect(box.rect.y).toBeGreaterThanOrEqual(0);
      expect(box.rect.x + box.rect.w).toBeLessThanOrEqual(1 + 1e-9);
      expect(box.rect.y + box.rect.h).toBeLessThanOrEqual(1 + 1e-9);
    }
    expect(map.mappedBytes).toBe(100);
  });

  it("names MAX_BOXES apps and folds the rest into one box", () => {
    const map = memoryMap(Array.from({ length: 30 }, (_, i) => app(`app${i}`, 1000)));
    expect(appBoxes(map.boxes)).toHaveLength(MAX_BOXES);
    const other = map.boxes.find((box) => box.kind === "other");
    expect(other?.kind).toBe("other");
    if (other?.kind !== "other") throw new Error("no other box");
    expect(other.apps).toHaveLength(30 - MAX_BOXES);
    expect(other.bytes).toBe((30 - MAX_BOXES) * 1000);
    expect(map.boxes.reduce((sum, box) => sum + box.bytes, 0)).toBe(map.mappedBytes);
  });

  it("folds an app below MIN_SHARE instead of drawing a sliver", () => {
    const small = Math.floor(200_000 * MIN_SHARE) - 1;
    const map = memoryMap([app("big", 200_000), app("small", small)]);
    expect(appBoxes(map.boxes).map((box) => box.app.key)).toEqual(["big"]);
    const other = map.boxes.find((box) => box.kind === "other");
    expect(other?.kind).toBe("other");
    if (other?.kind !== "other") throw new Error("no other box");
    expect(other.apps.map((entry) => entry.key)).toEqual(["small"]);
    expect(other.bytes).toBe(small);
  });

  it("keeps the folded apps on the other box, so the next level is the same apps", () => {
    const apps = Array.from({ length: 30 }, (_, i) => app(`app${i}`, 1000));
    const map = memoryMap(apps);
    const other = map.boxes.find((box) => box.kind === "other");
    if (other?.kind !== "other") throw new Error("no other box");
    expect(other.apps).toHaveLength(30 - MAX_BOXES);
    expect(other.apps.reduce((sum, app) => sum + app.memoryBytes, 0)).toBe(other.bytes);
    const next = memoryMap(other.apps);
    expect(next.mappedBytes).toBe(other.bytes);
    expect(next.boxes.every((box) => box.kind === "app")).toBe(true);
  });

  it("keeps the layout when every reading scales by the same factor", () => {
    const apps = [app("a", 500), app("b", 300), app("c", 150), app("d", 50)];
    const once = memoryMap(apps);
    const ten = memoryMap(apps.map((entry) => ({ ...entry, memoryBytes: entry.memoryBytes * 10 })));
    expect(ten.boxes.map((box) => box.rect)).toEqual(once.boxes.map((box) => box.rect));
  });

  it("returns an empty map when nothing was measured", () => {
    const empty = { boxes: [], mappedBytes: 0 };
    expect(memoryMap([])).toEqual(empty);
    expect(memoryMap([app("a", 0), app("b", 0)])).toEqual(empty);
    expect(memoryMap([app("nan", Number.NaN), app("inf", Number.POSITIVE_INFINITY)])).toEqual(empty);
  });

  it("keeps input order for equal apps", () => {
    expect(appBoxes(memoryMap([app("first", 100), app("second", 100)]).boxes).map((box) => box.app.key)).toEqual([
      "first",
      "second",
    ]);
    expect(appBoxes(memoryMap([app("second", 100), app("first", 100)]).boxes).map((box) => box.app.key)).toEqual([
      "second",
      "first",
    ]);
  });

  it("keeps the largest app when every app is below MIN_SHARE", () => {
    const map = memoryMap(Array.from({ length: 300 }, (_, i) => app(`app${i}`, 1000)));
    expect(appBoxes(map.boxes)).toHaveLength(1);
    expect(appBoxes(map.boxes)[0].app.key).toBe("app0");
    const other = map.boxes.find((box) => box.kind === "other");
    expect(other?.kind).toBe("other");
    if (other?.kind !== "other") throw new Error("no other box");
    expect(other.apps).toHaveLength(299);
    expect(other.bytes).toBe(299_000);
  });

  it("leaves every named box readable on a 200-app machine", () => {
    const apps = Array.from({ length: 200 }, (_, i) =>
      i < 20 ? app(`top${i}`, Math.round(1e9 * 0.87 ** i)) : app(`tail${i}`, 10_000_000),
    );
    const named = appBoxes(memoryMap(apps).boxes);
    expect(named).toHaveLength(MAX_BOXES);
    for (const box of named) {
      expect(Math.min(box.rect.w * 700, box.rect.h * 240)).toBeGreaterThanOrEqual(12);
    }
  });
});
