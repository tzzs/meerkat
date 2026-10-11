import { describe, expect, it } from "vitest";
import { MAX_PINS, parsePins, togglePin } from "./pins";

describe("parsePins", () => {
  it("reads back what was stored", () => {
    expect(parsePins(JSON.stringify(["c:\\a.exe", "c:\\b.exe"]))).toEqual(["c:\\a.exe", "c:\\b.exe"]);
  });

  it("reads anything malformed as no pins", () => {
    expect(parsePins(null)).toEqual([]);
    expect(parsePins("")).toEqual([]);
    expect(parsePins("{not json")).toEqual([]);
    expect(parsePins('{"a":1}')).toEqual([]);
  });

  it("keeps only non-empty strings, once each, and no more than the cap", () => {
    expect(parsePins(JSON.stringify(["a", 3, "", null, "a", "b"]))).toEqual(["a", "b"]);
    const many = Array.from({ length: MAX_PINS + 50 }, (_, i) => `k${i}`);
    expect(parsePins(JSON.stringify(many))).toHaveLength(MAX_PINS);
  });
});

describe("togglePin", () => {
  it("adds a new pin at the end and removes an existing one", () => {
    expect(togglePin(["a"], "b")).toEqual(["a", "b"]);
    expect(togglePin(["a", "b"], "a")).toEqual(["b"]);
  });
});
