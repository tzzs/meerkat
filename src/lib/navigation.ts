import { TAB_ORDER, type TabConfig } from "./types";

/** Where the window remembers the tab it was last left on, for "open on the last tab". */
export const LAST_TAB_KEY = "meerkat.lastTab";

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

/** The window has eight tabs, so Ctrl+1 to Ctrl+8 reach all of them. */
export const SHORTCUT_TABS = 8;

/**
 * The tab a digit key selects, counted along the tabs the bar is showing (a hidden tab takes no
 * number). Null for any key that is not one of the eight digits, and for a place the bar has no
 * tab in. The map used to stop at seven, so the Sound tab had no shortcut at all.
 */
export function tabForShortcut(key: string, shown: readonly string[]): string | null {
  return /^[1-8]$/.test(key) ? (shown[Number(key) - 1] ?? null) : null;
}

/** What the tab at this place in the bar says about its shortcut: "Ctrl+3", or nothing past the eighth. */
export function shortcutHint(index: number): string | null {
  return Number.isInteger(index) && index >= 0 && index < SHORTCUT_TABS ? `Ctrl+${index + 1}` : null;
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
