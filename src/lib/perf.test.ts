import { describe, expect, it } from "vitest";
import { defaultUnits, type UnitPrefs } from "./format";
import {
  formatPerfStat,
  formatPerfValue,
  perfAppChoices,
  perfHeadlineKey,
  perfIssueKey,
  perfIssueValue,
  perfSeries,
  perfTargetLabel,
  perfVerdictKey,
} from "./perf";
import type { AppKind, PerfIssue, PerfReport, PerfSample, PerfSession, PerfStat } from "./types";

const units: UnitPrefs = { ...defaultUnits, locale: "en-US" };

function sample(over: Partial<PerfSample> = {}): PerfSample {
  return {
    atMs: 1000,
    fps: 60,
    fpsSource: "presentMon",
    cpuPercent: 25,
    appCpuPercent: 10,
    cpuTempC: 68,
    gpuPercent: 40,
    appGpuPercent: 30,
    gpuTempC: 62,
    memoryPercent: 55,
    appMemoryBytes: 900_000_000,
    vramPercent: 50,
    vramUsedBytes: 2_000_000_000,
    vramTotalBytes: 4_000_000_000,
    ...over,
  };
}

function report(over: Partial<PerfReport> = {}): PerfReport {
  return {
    sessionId: "s1",
    verdict: "warning",
    headlineKey: "perf.headline.warning",
    issues: [],
    stats: [],
    durationSeconds: 30,
    sampleCount: 2,
    ...over,
  };
}

function stat(over: Partial<PerfStat> = {}): PerfStat {
  return { key: "fps", available: true, min: 30, avg: 50, max: 60, p95: 59, last: 55, ...over };
}

function finding(over: Partial<PerfIssue> = {}): PerfIssue {
  return {
    kind: "gpuBound",
    verdict: "problem",
    messageKey: "perf.issue.gpuBound",
    metric: "gpuPercent",
    value: 96,
    ...over,
  };
}

describe("verdict and finding words", () => {
  it("gives the user a readable key for each verdict", () => {
    expect(perfVerdictKey("healthy")).toBe("perf.verdict.healthy");
    expect(perfVerdictKey("warning")).toBe("perf.verdict.warning");
    expect(perfVerdictKey("problem")).toBe("perf.verdict.problem");
    expect(perfVerdictKey("inconclusive")).toBe("perf.verdict.inconclusive");
  });

  it("uses the backend headline when it is one Meerkat can say", () => {
    expect(perfHeadlineKey(report({ headlineKey: "perf.headline.problem" }))).toBe("perf.headline.problem");
    expect(perfHeadlineKey(report({ headlineKey: "untranslated key", verdict: "healthy" }))).toBe(
      "perf.headline.healthy",
    );
  });

  it("uses the backend finding sentence and falls back to the finding kind", () => {
    const issue = finding();
    expect(perfIssueKey(issue)).toBe("perf.issue.gpuBound");
    expect(perfIssueKey(finding({ messageKey: "" }))).toBe("perf.issue.gpuBound");
  });

  it("puts evidence beside a finding in the unit the reader expects", () => {
    expect(perfIssueValue(finding(), units)).toBe("96%");
    expect(perfIssueValue(finding({ kind: "fpsLow", metric: "fps", value: 42 }), units)).toBe("42 fps");
    expect(perfIssueValue(finding({ kind: "gpuHot", metric: "gpuTemp", value: 83 }), units)).toBe("83°C");
  });
});

describe("figures a reader can see", () => {
  it("prints each metric in its own unit", () => {
    expect(formatPerfValue(59.6, "fps", units)).toBe("60 fps");
    expect(formatPerfValue(63.4, "percent", units)).toBe("63%");
    expect(formatPerfValue(71, "celsius", units)).toBe("71°C");
    expect(formatPerfValue(2_000_000_000, "bytes", units)).toBe("2.00\u00a0GB");
  });

  it("shows a dash instead of a measurement nobody took", () => {
    expect(formatPerfValue(null, "fps", units)).toBe("—");
    expect(formatPerfValue(Number.NaN, "percent", units)).toBe("—");
    expect(formatPerfStat(stat({ available: false, last: 0 }), "percent", units)).toBe("—");
    expect(formatPerfStat(undefined, "celsius", units)).toBe("—");
  });

  it("keeps a measured zero visible as zero", () => {
    expect(formatPerfStat(stat({ last: 0 }), "percent", units)).toBe("0%");
  });
});

describe("a session's series", () => {
  const session: PerfSession = {
    id: "s1",
    target: { mode: "app", appKey: "game", name: "Game", currentAppKey: "game" },
    startedAtMs: 1000,
    endedAtMs: 4000,
    samples: [
      sample({ atMs: 1000, fps: 60, vramUsedBytes: 2_000_000_000 }),
      sample({ atMs: 2000, fps: null, fpsSource: "none", vramUsedBytes: null }),
      sample({ atMs: 3000, fps: 42, vramUsedBytes: 3_000_000_000 }),
    ],
    report: null,
  };

  it("skips samples where the metric was unavailable", () => {
    expect(perfSeries(session, "fps")).toEqual([60, 42]);
    expect(perfSeries(session, "cpuTempC")).toEqual([68, 68, 68]);
  });

  it("keeps VRAM bytes separate from the percentage", () => {
    expect(perfSeries(session, "vramPercent")).toEqual([50, 50, 50]);
    expect(perfSeries(session, "vramUsedBytes")).toEqual([2_000_000_000, 3_000_000_000]);
  });
});

describe("the target shown in a session", () => {
  it("uses the app's name when the backend has one", () => {
    expect(perfTargetLabel({ mode: "app", name: "  Game  " }, "Foreground window")).toBe("Game");
  });

  it("falls back to the mode label when no window could be named", () => {
    expect(perfTargetLabel({ mode: "foreground", name: "   " }, "Foreground window")).toBe("Foreground window");
  });
});

describe("the apps a session can watch", () => {
  const choice = (name: string, kind: AppKind, exePath = `C:\\Program Files\\${name}\\${name}.exe`) => ({
    key: exePath,
    name,
    kind,
    exePath,
  });

  it("offers programs and leaves services and system processes out", () => {
    const apps = [
      choice("Game", "app"),
      choice("Launcher", "packaged"),
      choice("Helper", "background"),
      choice("Print Spooler", "service"),
      choice("Registry", "system"),
    ];
    expect(perfAppChoices(apps, "").map((app) => app.name)).toEqual(["Game", "Helper", "Launcher"]);
  });

  it("matches an app by name or by executable path, ignoring case and surrounding spaces", () => {
    const apps = [choice("Game", "app", "C:\\Games\\game.exe"), choice("Editor", "app", "C:\\Apps\\Editor\\editor.exe")];
    expect(perfAppChoices(apps, "  GAM ").map((app) => app.name)).toEqual(["Game"]);
    expect(perfAppChoices(apps, "apps\\editor").map((app) => app.name)).toEqual(["Editor"]);
  });

  it("sorts by name and answers an empty list when nothing matches", () => {
    const apps = [choice("Zebra", "app"), choice("alpha", "app"), choice("Beta", "packaged")];
    expect(perfAppChoices(apps, "").map((app) => app.name)).toEqual(["alpha", "Beta", "Zebra"]);
    expect(perfAppChoices(apps, "nothing like this")).toEqual([]);
  });
});
