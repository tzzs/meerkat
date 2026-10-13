import type { Translate } from "../i18n";
import type { Key } from "../i18n/en";
import type { SystemEvent, SystemEventKind } from "./types";

/**
 * The kinds that mean something went wrong. Sleep, wake, start, shut down and an installed
 * update are the machine's ordinary life; they stay on the timeline as context, drawn quietly.
 */
const PROBLEMS: ReadonlySet<SystemEventKind> = new Set<SystemEventKind>([
  "appCrash",
  "appHang",
  "blueScreen",
  "powerLoss",
  "driverReset",
  "hardwareError",
  "updateFailed",
  "diskError",
  "serviceCrash",
]);

export function isProblem(event: Pick<SystemEvent, "kind">): boolean {
  return PROBLEMS.has(event.kind);
}

const TEXT: Record<SystemEventKind, Key> = {
  appCrash: "event.appCrash",
  appHang: "event.appHang",
  blueScreen: "event.blueScreen",
  powerLoss: "event.powerLoss",
  sleep: "event.sleep",
  wake: "event.wake",
  boot: "event.boot",
  shutdown: "event.shutdown",
  driverReset: "event.driverReset",
  hardwareError: "event.hardwareError",
  updateInstalled: "event.updateInstalled",
  updateFailed: "event.updateFailed",
  diskError: "event.diskError",
  serviceCrash: "event.serviceCrash",
};

/** One line saying what happened, in the window's language. */
export function eventText(event: SystemEvent, t: Translate): string {
  if (event.kind === "wake" && event.subject) return t("event.wakeBy", { subject: event.subject });
  const subject = event.subject || "—";
  const line = t(TEXT[event.kind], { subject, detail: event.detail || "—" });
  // An app crash's exception code is the thing to search for, and only worth its space there.
  if (event.kind === "appCrash" && event.detail) return `${line} · ${t("event.exceptionCode", { code: event.detail })}`;
  return line;
}
