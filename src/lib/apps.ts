import type { Translate } from "../i18n";
import type { Key } from "../i18n/en";
import { UNREADABLE_GROUP, type AppInfo } from "./types";

/**
 * The processes with no file behind them carry the kernel's bare names (`vmmemWSL`). Their
 * keys are fixed by the backend (`image:<name>`), so they can be given names people recognise.
 */
const PSEUDO: Record<string, Key> = {
  "image:system": "process.system",
  "image:registry": "process.registry",
  "image:memory compression": "process.memCompression",
  "image:memcompression": "process.memCompression",
  "image:secure system": "process.secureSystem",
  "image:vmmemwsl": "process.vmmemWsl",
  "image:vmmem": "process.vmmem",
};

/** What a row calls an app, in the language the window is in. */
export function appLabel(app: Pick<AppInfo, "key" | "name">, t: Translate): string {
  if (app.key === UNREADABLE_GROUP) return t("list.unreadable");
  const pseudo = PSEUDO[app.key];
  return pseudo ? t(pseudo) : app.name;
}

/** Settings → "Show Windows' own processes in lists": service hosts and the kernel's own. */
export function listedApps(apps: AppInfo[], showSystem: boolean): AppInfo[] {
  return showSystem ? apps : apps.filter((app) => app.kind !== "system" && app.kind !== "service");
}

/**
 * The services an app runs, as a short note for the row's second line. A service host is
 * already named after its first service, so only the rest are counted.
 */
export function serviceNote(app: Pick<AppInfo, "name" | "services">, t: Translate): string {
  const rest = app.services.filter((service) => service !== app.name);
  if (rest.length === 0) return "";
  if (rest.length < app.services.length) return t("list.moreServices", { count: rest.length });
  return rest.length === 1 ? rest[0] : `${rest[0]} ${t("list.moreServices", { count: rest.length - 1 })}`;
}

/** The hover text of a row: its windows or its file, the services in it, and why it has no ×. */
export function appHint(app: AppInfo, t: Translate): string {
  if (app.key === UNREADABLE_GROUP) return t("list.unreadableHint");
  const lines: string[] = app.windows.length > 0 ? app.windows.map((w) => w.title) : app.exePath ? [app.exePath] : [];
  if (app.services.length > 0) lines.push("", t("list.servicesHeading"), ...app.services);
  if (!app.canQuit) lines.push("", t("list.cannotQuit"));
  return lines.join("\n").trim();
}
