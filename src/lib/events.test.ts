import { describe, expect, it } from "vitest";
import { makeT } from "../i18n";
import { eventText, isProblem } from "./events";
import type { SystemEvent } from "./types";

const zh = makeT("zh-Hans");
const en = makeT("en");
const event = (over: Partial<SystemEvent>): SystemEvent => ({ atMs: 0, kind: "boot", subject: "", detail: "", ...over });

describe("eventText", () => {
  it("names the app that crashed and its exception code", () => {
    expect(eventText(event({ kind: "appCrash", subject: "game.exe", detail: "c0000005" }), zh)).toBe(
      "game.exe 崩溃了 · 异常代码 c0000005",
    );
  });

  it("says what woke the machine when Windows recorded it", () => {
    expect(eventText(event({ kind: "wake", subject: "USB Mouse" }), en)).toBe("Woken by USB Mouse");
    expect(eventText(event({ kind: "wake" }), en)).toBe("Woke up");
  });

  it("puts the stop code in a blue screen's line", () => {
    expect(eventText(event({ kind: "blueScreen", detail: "0x0000009F" }), zh)).toContain("0x0000009F");
  });

  it("does not print an empty placeholder", () => {
    expect(eventText(event({ kind: "diskError" }), en)).not.toContain("{");
  });
});

describe("isProblem", () => {
  it("separates trouble from the machine's ordinary life", () => {
    expect(isProblem({ kind: "driverReset" })).toBe(true);
    expect(isProblem({ kind: "sleep" })).toBe(false);
    expect(isProblem({ kind: "updateInstalled" })).toBe(false);
  });
});
