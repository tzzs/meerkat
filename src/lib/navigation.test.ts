import { describe, expect, it } from "vitest";
import { LAST_TAB_KEY, openingTab, readLastTab, shortcutHint, tabForShortcut, writeLastTab } from "./navigation";
import { TAB_ORDER } from "./types";

const tabs = TAB_ORDER.map((id) => ({ id, visible: id !== "gpu", sections: [] }));

describe("openingTab", () => {
  it("opens the tab the setting names", () => {
    expect(openingTab("memory", null, tabs)).toBe("memory");
  });

  it("opens the remembered tab for 'last'", () => {
    expect(openingTab("last", "disk", tabs)).toBe("disk");
  });

  it("falls back to Overview when nothing was remembered", () => {
    expect(openingTab("last", null, tabs)).toBe("overview");
  });

  it("never opens on a tab that does not exist or is hidden", () => {
    expect(openingTab("projects", null, tabs)).toBe("overview");
    expect(openingTab("last", "settings", tabs)).toBe("overview");
    expect(openingTab("gpu", null, tabs)).toBe("overview");
    expect(openingTab("last", "gpu", tabs)).toBe("overview");
  });
});

describe("tabForShortcut", () => {
  const shown = TAB_ORDER.filter((id) => id !== "gpu");

  it("counts along the tabs the bar shows, so a hidden tab takes no number", () => {
    expect(tabForShortcut("1", shown)).toBe("overview");
    expect(tabForShortcut("2", shown)).toBe("history");
    expect(tabForShortcut("7", shown)).toBe("battery");
    // With the GPU tab hidden, what was 9 is 8, and Sound moves up to 8.
    expect(tabForShortcut("8", shown)).toBe("sound");
  });

  it("reaches all nine tabs", () => {
    expect(tabForShortcut("9", TAB_ORDER)).toBe("sound");
    expect(tabForShortcut("8", TAB_ORDER)).toBe("battery");
  });

  it("has nothing for a place the bar does not have, or a key that is not a digit", () => {
    expect(tabForShortcut("9", shown)).toBeNull();
    expect(tabForShortcut("0", TAB_ORDER)).toBeNull();
    expect(tabForShortcut("", TAB_ORDER)).toBeNull();
    expect(tabForShortcut("Control", TAB_ORDER)).toBeNull();
    expect(tabForShortcut("12", TAB_ORDER)).toBeNull();
  });
});

describe("shortcutHint", () => {
  it("says the key that picks the tab at that place", () => {
    expect(shortcutHint(0)).toBe("Ctrl+1");
    expect(shortcutHint(7)).toBe("Ctrl+8");
    expect(shortcutHint(8)).toBe("Ctrl+9");
  });

  it("says nothing where there is no shortcut", () => {
    expect(shortcutHint(9)).toBeNull();
    expect(shortcutHint(-1)).toBeNull();
  });
});

describe("the remembered tab", () => {
  it("round-trips through storage under its own key", () => {
    const map = new Map<string, string>();
    const store = { getItem: (k: string) => map.get(k) ?? null, setItem: (k: string, v: string) => void map.set(k, v) };
    writeLastTab("network", store);
    expect(map.get(LAST_TAB_KEY)).toBe("network");
    expect(readLastTab(store)).toBe("network");
  });

  it("treats storage that throws as storage that is empty", () => {
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("quota");
      },
    };
    expect(readLastTab(broken)).toBeNull();
    expect(() => writeLastTab("cpu", broken)).not.toThrow();
    expect(readLastTab(null)).toBeNull();
  });
});
