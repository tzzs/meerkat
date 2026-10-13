import type { AppInfo, ListeningPort } from "./types";

export type PortScope = "network" | "local" | "address";

export interface PortRow {
  port: number;
  app: AppInfo | null;
  pid: number;
  /** "network" when any of its sockets takes connections from other machines. */
  scope: PortScope;
  /** The specific address, when it is neither every interface nor loopback. */
  address: string;
}

const ANY = new Set(["0.0.0.0", "::"]);
const LOOPBACK = new Set(["127.0.0.1", "::1"]);

/**
 * One row per port and process. A server usually binds the same port on IPv4 and IPv6, which
 * the table lists twice; what matters is whether any of those sockets can be reached from
 * another machine.
 */
export function portRows(listening: ListeningPort[], apps: AppInfo[]): PortRow[] {
  const byKey = new Map(apps.map((app) => [app.key, app]));
  const rows = new Map<string, PortRow>();
  for (const socket of listening) {
    const id = `${socket.port}:${socket.pid}`;
    const scope: PortScope = ANY.has(socket.address) ? "network" : LOOPBACK.has(socket.address) ? "local" : "address";
    const existing = rows.get(id);
    if (existing) {
      if (scope === "network" || (scope === "address" && existing.scope === "local")) {
        existing.scope = scope;
        existing.address = socket.address;
      }
      continue;
    }
    rows.set(id, { port: socket.port, app: byKey.get(socket.appKey) ?? null, pid: socket.pid, scope, address: socket.address });
  }
  return [...rows.values()].sort((a, b) => a.port - b.port || a.pid - b.pid);
}

/** A port number typed in matches by prefix; anything else matches the app's name or project. */
export function portMatches(row: PortRow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  if (/^\d+$/.test(needle)) return String(row.port).startsWith(needle);
  const app = row.app;
  return Boolean(
    app && (app.name.toLowerCase().includes(needle) || (app.project ?? "").toLowerCase().includes(needle) || app.exePath.toLowerCase().includes(needle)),
  );
}
