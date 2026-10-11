import { describe, expect, it } from "vitest";
import { nudge, parseNumberInput, readNumber, type NumberRule } from "./number-input";

const percent: NumberRule = { min: 1, max: 100, step: 1, integer: true };

describe("parseNumberInput", () => {
  it("accepts a number inside the range", () => {
    expect(parseNumberInput("25", percent)).toEqual({ ok: true, value: 25 });
    expect(parseNumberInput("1", percent)).toEqual({ ok: true, value: 1 });
    expect(parseNumberInput("100", percent)).toEqual({ ok: true, value: 100 });
    expect(parseNumberInput("  40  ", percent)).toEqual({ ok: true, value: 40 });
  });

  // An emptied field used to be Number("") = 0, which saved a CPU threshold of 0% and made
  // every application count as misbehaving.
  it("does not turn an emptied field into zero", () => {
    expect(parseNumberInput("", percent)).toEqual({ ok: false, problem: "empty" });
    expect(parseNumberInput("   ", percent)).toEqual({ ok: false, problem: "empty" });
  });

  it("refuses what is outside the range, at either end", () => {
    expect(parseNumberInput("0", percent)).toEqual({ ok: false, problem: "tooLow" });
    expect(parseNumberInput("-5", percent)).toEqual({ ok: false, problem: "tooLow" });
    expect(parseNumberInput("101", percent)).toEqual({ ok: false, problem: "tooHigh" });
  });

  it("refuses text that is not a plain decimal", () => {
    for (const text of ["abc", "1e3", "0x10", "Infinity", "NaN", "1,5", "5%", "--3", "1 2"]) {
      expect(parseNumberInput(text, percent)).toEqual({ ok: false, problem: "notNumber" });
    }
  });

  it("refuses a fraction where only whole numbers are meaningful", () => {
    expect(parseNumberInput("12.5", percent)).toEqual({ ok: false, problem: "notWhole" });
    expect(parseNumberInput("12.0", percent)).toEqual({ ok: true, value: 12 });
    expect(parseNumberInput("12.5", { ...percent, integer: false })).toEqual({ ok: true, value: 12.5 });
  });

  it("reads full-width digits typed through an input method", () => {
    expect(parseNumberInput("３０", percent)).toEqual({ ok: true, value: 30 });
  });

  it("does not accept a number too long to be one", () => {
    expect(parseNumberInput("9".repeat(400), percent)).toEqual({ ok: false, problem: "notNumber" });
  });
});

describe("readNumber", () => {
  it("returns null rather than NaN", () => {
    expect(readNumber("")).toBeNull();
    expect(readNumber("x")).toBeNull();
    expect(readNumber("7")).toBe(7);
    expect(readNumber(".5")).toBe(0.5);
  });
});

describe("nudge", () => {
  const seconds: NumberRule = { min: 5, max: 3600, step: 5, integer: true };

  it("moves one step either way", () => {
    expect(nudge("30", 1, seconds)).toBe("35");
    expect(nudge("30", -1, seconds)).toBe("25");
  });

  it("stays inside the rule", () => {
    expect(nudge("3600", 1, seconds)).toBe("3600");
    expect(nudge("5", -1, seconds)).toBe("5");
    expect(nudge("2", -1, seconds)).toBe("5");
  });

  it("starts from the lowest allowed value when the text is not a number", () => {
    expect(nudge("", 1, seconds)).toBe("5");
    expect(nudge("abc", -1, seconds)).toBe("5");
  });
});
