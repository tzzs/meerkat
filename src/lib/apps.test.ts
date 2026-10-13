import { describe, expect, it } from "vitest";
import { makeT } from "../i18n";
import { appHint, appLabel, listedApps, serviceNote } from "./apps";
import type { AppInfo } from "./types";

const zh = makeT("zh-Hans");
const en = makeT("en");

function app(over: Partial<AppInfo>): AppInfo {
  return {
    key: "c:\\program files\\app\\app.exe",
    name: "App",
    exePath: "C:\\Program Files\\App\\app.exe",
    kind: "app",
    mainPid: 100,
    processCount: 1,
    cpuPercent: 0,
    memoryBytes: 0,
    readBytesPerSecond: 0,
    writeBytesPerSecond: 0,
    readBytesTotal: 0,
    writeBytesTotal: 0,
    connections: 0,
    listeningPorts: [],
    oldestStartedAtMs: null,
    windows: [],
    pids: [100],
    unmeasuredProcesses: 0,
    services: [],
    canQuit: true,
    control: { efficiency: false, priority: null, muteAlerts: false, paused: false },
    project: null,
    ...over,
  };
}

describe("appLabel", () => {
  it("names the processes that have no file the way Task Manager does", () => {
    expect(appLabel(app({ key: "image:memory compression", name: "Memory Compression" }), zh)).toBe("内存压缩");
    expect(appLabel(app({ key: "image:vmmemwsl", name: "vmmemWSL" }), zh)).toBe("WSL 虚拟机");
    expect(appLabel(app({ key: "image:system", name: "System" }), en)).toBe("System (kernel and drivers)");
  });

  it("leaves every other name as Windows gave it", () => {
    expect(appLabel(app({ name: "Windows 更新" }), en)).toBe("Windows 更新");
  });
});

describe("listedApps", () => {
  const list = [app({ kind: "app" }), app({ kind: "service" }), app({ kind: "system" }), app({ kind: "background" })];

  it("keeps everything when the setting says to show Windows' own processes", () => {
    expect(listedApps(list, true)).toHaveLength(4);
  });

  it("drops services and the system otherwise", () => {
    expect(listedApps(list, false).map((a) => a.kind)).toEqual(["app", "background"]);
  });
});

describe("serviceNote", () => {
  it("says nothing for a host named after its only service", () => {
    expect(serviceNote(app({ name: "Windows 更新", services: ["Windows 更新"] }), zh)).toBe("");
  });

  it("counts the services after the one the row is named for", () => {
    expect(serviceNote(app({ name: "A", services: ["A", "B", "C"] }), zh)).toBe("另 2 项服务");
    expect(serviceNote(app({ name: "A", services: ["A", "B"] }), en)).toBe("+1 more service");
  });

  it("names the service of a program that is not called after it", () => {
    const defender = app({ name: "Antimalware Service Executable", services: ["Microsoft Defender Antivirus Service"] });
    expect(serviceNote(defender, en)).toBe("Microsoft Defender Antivirus Service");
  });
});

describe("appHint", () => {
  it("explains why a system row offers no way to end it", () => {
    const hint = appHint(app({ canQuit: false, services: ["Windows Update"] }), en);
    expect(hint).toContain("Windows Update");
    expect(hint).toContain("administrator rights");
  });

  it("lists window titles before anything else", () => {
    expect(appHint(app({ windows: [{ title: "Doc 1", pid: 1, visible: true, hwnd: 1 }] }), en)).toBe("Doc 1");
  });
});
