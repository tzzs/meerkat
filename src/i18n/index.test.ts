import { describe, expect, it } from "vitest";
import { makeT } from "./index";

describe("translations with parameters", () => {
  // Every plural form contains a space, and the parser used to split the forms on whitespace,
  // so each of these rendered as an empty string in both languages.
  it("picks the English plural form by count", () => {
    const t = makeT("en");
    expect(t("time.minutesAgo", { count: 1 })).toBe("1 minute ago");
    expect(t("time.minutesAgo", { count: 5 })).toBe("5 minutes ago");
    expect(t("label.adapters", { count: 2 })).toBe("2 adapters");
  });

  it("uses the only form Chinese has", () => {
    const t = makeT("zh-Hans");
    expect(t("action.confirmQuitBody", { count: 3 })).toBe("3 个进程将被要求关闭，未保存的内容会丢失。");
    expect(t("overview.insightCount", { count: 2 })).toBe("2 项值得关注");
  });

  it("fills plain placeholders next to a plural one", () => {
    const t = makeT("en");
    expect(t("cpu.perCoreAverage", { average: "40%", cores: 20 })).toBe("40% average across 20 logical cores");
  });

  it("leaves a missing parameter empty rather than printing the placeholder", () => {
    expect(makeT("en")("alert.for", {})).toBe("held for ");
  });
});
