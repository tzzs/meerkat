import { describe, expect, it } from "vitest";
import { LAST_TAB_KEY, openingTab, readLastTab, writeLastTab } from "./navigation";
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
