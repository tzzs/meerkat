import { describe, expect, it } from "vitest";
import { clip, createLimiter, describeError } from "./report";

describe("createLimiter", () => {
  const limiter = () => createLimiter({ burst: 3, windowMs: 10_000, repeatMs: 60_000 });

  it("lets a burst through and then holds the rest until the window passes", () => {
    const allow = limiter();
    expect([1, 2, 3, 4].map((n) => allow(`error ${n}`, 0))).toEqual([true, true, true, false]);
    expect(allow("error 5", 9_999)).toBe(false);
    expect(allow("error 6", 10_000)).toBe(true);
  });

  // An error thrown on every animation frame must not become sixty log lines a second.
  it("sends the same message once per repeat interval", () => {
    const allow = limiter();
    expect(allow("same", 0)).toBe(true);
    expect(allow("same", 30_000)).toBe(false);
    expect(allow("same", 60_000)).toBe(true);
  });
});

describe("describeError", () => {
  it("keeps the stack of an Error and does not repeat its first line", () => {
    const error = new TypeError("x is undefined");
    const text = describeError(error);
    expect(text.startsWith("TypeError: x is undefined")).toBe(true);
    expect(text.indexOf("TypeError: x is undefined", 1)).toBe(-1);
  });

  it("writes out what is not an Error", () => {
    expect(describeError("plain")).toBe("plain");
    expect(describeError({ code: 5 })).toBe('{"code":5}');
    expect(describeError(undefined)).toBe("undefined");
  });

  it("survives a value that cannot be serialised", () => {
    const loop: Record<string, unknown> = {};
    loop.self = loop;
    expect(describeError(loop)).toBe("[object Object]");
  });
});

describe("clip", () => {
  it("cuts long reports and marks the cut", () => {
    expect(clip("abcdef", 4)).toBe("abc…");
    expect(clip("abc", 4)).toBe("abc");
  });
});
