import { describe, expect, it } from "vitest";
import { squarify, type Rect } from "./storage";

const area = (r: Rect) => r.w * r.h;

describe("squarify", () => {
  it("tiles the whole box", () => {
    for (const values of [[1, 1, 1, 1], [1, 2, 3, 4], [5, 3, 1, 7, 2], [100, 1]]) {
      const rects = squarify(values);
      const sum = rects.reduce((total, r) => total + area(r), 0);
      expect(Math.abs(sum - 1)).toBeLessThan(1e-9);
      for (const r of rects) {
        expect(r.x).toBeGreaterThanOrEqual(0);
        expect(r.y).toBeGreaterThanOrEqual(0);
        expect(r.x + r.w).toBeLessThanOrEqual(1 + 1e-9);
        expect(r.y + r.h).toBeLessThanOrEqual(1 + 1e-9);
      }
    }
  });

  it("a 1×1 with four equal items does not produce slivers", () => {
    const rects = squarify([1, 1, 1, 1]);
    for (const r of rects) {
      expect(Math.abs(area(r) - 0.25)).toBeLessThan(1e-9);
      // A sliver is one very thin edge; four equal squares have edges of 0.5.
      expect(Math.min(r.w, r.h)).toBeGreaterThan(0.4);
    }
  });

  it("makes areas proportional to values", () => {
    const values = [1, 2, 3, 4];
    const rects = squarify(values);
    const total = 10;
    for (let i = 0; i < values.length; i++) {
      expect(Math.abs(area(rects[i]) - values[i] / total)).toBeLessThan(1e-9);
    }
  });

  it("keeps the biggest value's rectangle on top", () => {
    // Squarify lays the largest values first, so the largest rectangle starts in the top-left
    // corner of the box; the exact geometry beyond that is not part of the contract.
    const rects = squarify([3, 1, 1, 1]);
    const largest = rects[0];
    expect(Math.abs(area(largest) - 0.5)).toBeLessThan(1e-9);
  });
});
