import { describe, expect, it } from "vitest";
import { portMatches, portRows } from "./ports";
import type { AppInfo, ListeningPort } from "./types";

const app = (key: string, name: string, project: string | null = null) =>
  ({ key, name, project, exePath: "C:\\x\\" + name + ".exe" }) as AppInfo;
const socket = (port: number, address: string, pid: number, appKey: string): ListeningPort => ({ port, address, pid, appKey });

describe("portRows", () => {
  const apps = [app("vite", "site · node", "E:\\work\\site"), app("pg", "postgres")];

  it("folds the IPv4 and IPv6 sockets of one server into one row", () => {
    const rows = portRows([socket(1420, "127.0.0.1", 10, "vite"), socket(1420, "::1", 10, "vite")], apps);
    expect(rows).toHaveLength(1);
    expect(rows[0].scope).toBe("local");
  });

  it("calls a port reachable from the network when any of its sockets is", () => {
    const rows = portRows([socket(5432, "127.0.0.1", 20, "pg"), socket(5432, "::", 20, "pg")], apps);
    expect(rows[0].scope).toBe("network");
  });

  it("keeps a specific address when it is the only one", () => {
    const rows = portRows([socket(8080, "192.168.1.5", 30, "pg")], apps);
    expect(rows[0]).toMatchObject({ scope: "address", address: "192.168.1.5" });
  });

  it("sorts by port and finds the app behind each", () => {
    const rows = portRows([socket(5432, "::", 20, "pg"), socket(1420, "::1", 10, "vite")], apps);
    expect(rows.map((r) => r.port)).toEqual([1420, 5432]);
    expect(rows[0].app?.name).toBe("site · node");
  });
});

describe("portMatches", () => {
  const [row] = portRows([socket(1420, "::1", 10, "vite")], [app("vite", "site · node", "E:\\work\\site")]);

  it("matches a number by prefix and words by app or project", () => {
    expect(portMatches(row, "14")).toBe(true);
    expect(portMatches(row, "42")).toBe(false);
    expect(portMatches(row, "site")).toBe(true);
    expect(portMatches(row, "work")).toBe(true);
    expect(portMatches(row, "chrome")).toBe(false);
  });
});
