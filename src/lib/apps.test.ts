import { describe, expect, it } from "vitest";
import { makeT } from "../i18n";
import {
  appLabel,
  canQuit,
  forLists,
  holdOrder,
  orderApps,
  processName,
  rowSignature,
  serviceHostRole,
  visibleWindowTitles,
} from "./apps";
import { UNREADABLE_GROUP, type AppInfo } from "./types";

const app = (key: string, memoryBytes: number, over: Partial<AppInfo> = {}): AppInfo =>
  ({
    key,
    name: key.toUpperCase(),
    exePath: "",
    kind: "app",
    memoryBytes,
    cpuPercent: 0,
    readBytesPerSecond: 0,
    writeBytesPerSecond: 0,
    connections: 0,
    windows: [],
    ...over,
  }) as AppInfo;

const keys = (list: AppInfo[]) => list.map((a) => a.key).join(" ");
const six = [app("a", 60), app("b", 50), app("c", 40), app("d", 30), app("e", 20), app("f", 10)];

describe("orderApps", () => {
  // The old comparator gave "d a b c e f": the second pin collided with "not pinned".
  it("keeps every pin on top, in pin order", () => {
    expect(keys(orderApps(six, ["d", "e"], "memory"))).toBe("d e a b c f");
    expect(keys(orderApps(six, ["e", "d"], "memory"))).toBe("e d a b c f");
  });

  it("orders the rest by the metric, largest first", () => {
    const shuffled = [six[3], six[0], six[5], six[1]];
    expect(keys(orderApps(shuffled, [], "memory"))).toBe("a b d f");
  });

  it("keeps ties in the order they arrived", () => {
    const tied = [app("x", 5), app("y", 5), app("z", 5)];
    expect(keys(orderApps(tied, [], "memory"))).toBe("x y z");
  });

  it("ignores pins for apps that are not in the list", () => {
    expect(keys(orderApps(six.slice(0, 3), ["gone", "c"], "memory"))).toBe("c a b");
  });

  it("does not reorder its input", () => {
    const input = [...six];
    orderApps(input, ["f"], "memory");
    expect(keys(input)).toBe("a b c d e f");
  });
});

describe("holdOrder", () => {
  it("keeps the held order while the figures change", () => {
    const resorted = orderApps([app("a", 1), app("b", 99), app("c", 50)], [], "memory");
    expect(keys(holdOrder(resorted, ["a", "b", "c"]))).toBe("a b c");
  });

  it("drops apps that went away and appends new ones", () => {
    expect(keys(holdOrder([app("b", 1), app("new", 9)], ["a", "b"]))).toBe("b new");
  });

  it("is the sorted order when nothing is held", () => {
    expect(keys(holdOrder(six, null))).toBe("a b c d e f");
  });
});

describe("labels", () => {
  const t = makeT("en");
  const z = makeT("zh-Hans");

  it("finds the role of a service host", () => {
    expect(serviceHostRole("c:\\windows\\system32\\svchost.exe|netsvcs")).toBe("netsvcs");
    expect(serviceHostRole("c:\\windows\\system32\\notepad.exe")).toBeNull();
    expect(serviceHostRole("c:\\tools\\notsvchost.exe|x")).toBeNull();
  });

  it("names a service host in the user's language", () => {
    const host = { key: "c:\\windows\\system32\\svchost.exe|LocalService", name: "Service host: LocalService" };
    expect(appLabel(host, t)).toBe("Service host: LocalService");
    expect(appLabel(host, z)).toBe("服务主机: LocalService");
  });

  it("names the folded row of unreadable processes", () => {
    expect(appLabel({ key: UNREADABLE_GROUP, name: "" }, t)).toBe(t("list.unreadable"));
  });

  it("keeps an ordinary app's own name", () => {
    expect(appLabel({ key: "c:\\x\\code.exe", name: "Visual Studio Code" }, t)).toBe("Visual Studio Code");
  });

  it("copies the executable's file name, or the app name without a path", () => {
    expect(processName({ exePath: "C:\\Program Files\\Google\\Chrome\\chrome.exe", name: "Chrome" })).toBe("chrome.exe");
    expect(processName({ exePath: "", name: "System" })).toBe("System");
  });
});

describe("canQuit", () => {
  it("offers to close apps but not Windows itself or the unreadable row", () => {
    expect(canQuit({ key: "x", kind: "app" })).toBe(true);
    expect(canQuit({ key: "x", kind: "background" })).toBe(true);
    expect(canQuit({ key: "x", kind: "system" })).toBe(false);
    expect(canQuit({ key: UNREADABLE_GROUP, kind: "background" })).toBe(false);
  });
});

describe("visibleWindowTitles", () => {
  it("lists only windows a person can see, once each", () => {
    const windows = [
      { title: "Inbox", visible: true, pid: 1, hwnd: 1 },
      { title: "MSCTFIME UI", visible: false, pid: 1, hwnd: 2 },
      { title: "Inbox", visible: true, pid: 1, hwnd: 3 },
      { title: " ", visible: true, pid: 1, hwnd: 4 },
    ];
    expect(visibleWindowTitles({ windows })).toEqual(["Inbox"]);
  });
});

describe("rowSignature", () => {
  const base = app("c:\\a\\a.exe", 100, {
    exePath: "c:\\a\\a.exe",
    processCount: 3,
    listeningPorts: [80, 443],
    windows: [{ title: "Doc", pid: 1, visible: true, hwnd: 1 }],
  });

  // Every sample builds new AppInfo objects, so the row can only be skipped by what it draws.
  it("does not move with the figures, which a row receives already formatted", () => {
    const later = { ...base, cpuPercent: 12, memoryBytes: 999, readBytesPerSecond: 5, connections: 9 };
    expect(rowSignature(later)).toBe(rowSignature(base));
  });

  it("moves with anything the row draws or hands to its menu", () => {
    const before = rowSignature(base);
    expect(rowSignature({ ...base, processCount: 4 })).not.toBe(before);
    expect(rowSignature({ ...base, listeningPorts: [80] })).not.toBe(before);
    expect(rowSignature({ ...base, kind: "service" })).not.toBe(before);
    expect(rowSignature({ ...base, name: "Renamed" })).not.toBe(before);
    expect(rowSignature({ ...base, exePath: "c:\\b\\b.exe" })).not.toBe(before);
    expect(rowSignature({ ...base, windows: [{ title: "Other", pid: 1, visible: true, hwnd: 1 }] })).not.toBe(before);
  });

  it("ignores windows nobody can see, as the hover list does", () => {
    const hidden = { ...base, windows: [...base.windows, { title: "Tool", pid: 1, visible: false, hwnd: 2 }] };
    expect(rowSignature(hidden)).toBe(rowSignature(base));
  });
});

describe("forLists", () => {
  it("hides system and service processes only when asked to", () => {
    const list = [app("a", 1), app("s", 1, { kind: "system" }), app("v", 1, { kind: "service" })];
    expect(keys(forLists(list, true))).toBe("a s v");
    expect(keys(forLists(list, false))).toBe("a");
  });
});
