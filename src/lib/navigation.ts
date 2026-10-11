import { TAB_ORDER, type TabConfig } from "./types";

/** Where the window remembers the tab it was last left on, for "open on the last tab". */
export const LAST_TAB_KEY = "pulse.lastTab";

type Reader = Pick<Storage, "getItem">;
type Writer = Pick<Storage, "setItem">;

/** localStorage, or nothing: a locked-down profile throws on the mere property access. */
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readLastTab(from: Reader | null = storage()): string | null {
  try {
    return from?.getItem(LAST_TAB_KEY) ?? null;
  } catch {
    return null;
  }
}

export function writeLastTab(tab: string, to: Writer | null = storage()): void {
  try {
    to?.setItem(LAST_TAB_KEY, tab);
  } catch {
    // Storage full or disabled: the window opens on Overview next time, which is the default.
  }
}

export function isTabId(id: string | null | undefined): id is (typeof TAB_ORDER)[number] {
  return typeof id === "string" && (TAB_ORDER as readonly string[]).includes(id);
}

/**
 * The tab the window opens on. `openWindowOn` is a tab id or "last"; anything that is not a
 * tab the window currently shows — an id from an older version, a tab the user hid, "last"
 * with nothing remembered — opens Overview rather than an empty page.
 */
export function openingTab(openWindowOn: string, lastTab: string | null, tabs: readonly TabConfig[]): string {
  const wanted = openWindowOn === "last" ? lastTab : openWindowOn;
  if (!isTabId(wanted)) return "overview";
  return tabs.some((tab) => tab.id === wanted && tab.visible) ? wanted : "overview";
}
