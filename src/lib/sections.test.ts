import { describe, expect, it } from "vitest";
import { dict } from "../i18n/en";
import {
  TAB_SECTIONS,
  effectiveSections,
  moveWithin,
  sectionLabelKey,
  visibleSections,
  withTabSections,
} from "./sections";
import type { SectionConfig, Settings } from "./types";

/** Only the part of the settings these functions read; the store module needs a browser. */
const TABS = ["overview", "cpu", "memory", "disk", "network", "gpu", "battery", "sound"];
const fallbackSettings: Pick<Settings, "window"> = {
  window: { tabs: TABS.map((id) => ({ id, visible: true, sections: [] as SectionConfig[] })) },
};

const withSections = (tab: string, sections: SectionConfig[]): Pick<Settings, "window"> => ({
  window: {
    tabs: fallbackSettings.window.tabs.map((t) => (t.id === tab ? { ...t, sections } : t)),
  },
});

describe("effectiveSections", () => {
  it("is every section, in design order, when nothing is saved", () => {
    expect(effectiveSections(undefined, "cpu")).toEqual([
      { id: "perCore", visible: true },
      { id: "history", visible: true },
      { id: "apps", visible: true },
    ]);
    expect(effectiveSections({ sections: [] }, "cpu").map((s) => s.id)).toEqual(TAB_SECTIONS.cpu);
  });

  it("keeps the saved order and visibility", () => {
    const saved = [
      { id: "apps", visible: true },
      { id: "perCore", visible: false },
      { id: "history", visible: true },
    ];
    expect(effectiveSections({ sections: saved }, "cpu")).toEqual(saved);
  });

  it("appends a section a saved list does not know about, visible", () => {
    const saved = [{ id: "history", visible: true }];
    expect(effectiveSections({ sections: saved }, "cpu")).toEqual([
      { id: "history", visible: true },
      { id: "perCore", visible: true },
      { id: "apps", visible: true },
    ]);
  });

  it("drops ids this build has no section for, and repeats", () => {
    const saved = [
      { id: "nonsense", visible: true },
      { id: "apps", visible: false },
      { id: "apps", visible: true },
    ];
    const out = effectiveSections({ sections: saved }, "cpu");
    expect(out.map((s) => s.id)).toEqual(["apps", "perCore", "history"]);
    expect(out[0].visible).toBe(false);
  });

  it("has nothing for a tab it does not know, including an object property name", () => {
    expect(effectiveSections(undefined, "nope")).toEqual([]);
    expect(effectiveSections(undefined, "constructor")).toEqual([]);
  });
});

describe("visibleSections", () => {
  it("honours hiding and the saved order", () => {
    const settings = withSections("cpu", [
      { id: "apps", visible: true },
      { id: "perCore", visible: false },
      { id: "history", visible: true },
    ]);
    expect(visibleSections(settings, "cpu")).toEqual(["apps", "history"]);
  });

  it("shows everything for a tab with no saved list", () => {
    expect(visibleSections(fallbackSettings, "network")).toEqual(["adapters", "history", "connections"]);
  });

  it("has every section the tabs draw", () => {
    for (const tab of fallbackSettings.window.tabs) {
      expect(visibleSections(fallbackSettings, tab.id).length).toBe(TAB_SECTIONS[tab.id].length);
    }
  });
});

describe("moveWithin", () => {
  const list = [{ id: "a" }, { id: "b" }, { id: "c" }];

  it("swaps with the neighbour", () => {
    expect(moveWithin(list, "b", -1).map((x) => x.id)).toEqual(["b", "a", "c"]);
    expect(moveWithin(list, "b", 1).map((x) => x.id)).toEqual(["a", "c", "b"]);
  });

  it("stays put at either end and for an unknown id", () => {
    expect(moveWithin(list, "a", -1).map((x) => x.id)).toEqual(["a", "b", "c"]);
    expect(moveWithin(list, "c", 1).map((x) => x.id)).toEqual(["a", "b", "c"]);
    expect(moveWithin(list, "z", 1).map((x) => x.id)).toEqual(["a", "b", "c"]);
  });

  it("steps over items that are not movable instead of swapping with them", () => {
    const tiles = [{ id: "cpu" }, { id: "sound" }, { id: "memory" }, { id: "disk" }];
    const isMetric = (item: { id: string }) => item.id !== "sound";
    expect(moveWithin(tiles, "memory", -1, isMetric).map((x) => x.id)).toEqual(["memory", "sound", "cpu", "disk"]);
    expect(moveWithin(tiles, "cpu", 1, isMetric).map((x) => x.id)).toEqual(["memory", "sound", "cpu", "disk"]);
  });

  it("does not change the list it was given", () => {
    moveWithin(list, "b", 1);
    expect(list.map((x) => x.id)).toEqual(["a", "b", "c"]);
  });
});

describe("withTabSections", () => {
  it("replaces one tab's list and nothing else", () => {
    const next = withTabSections(fallbackSettings, "disk", [{ id: "history", visible: false }]);
    expect(next.window.tabs.find((t) => t.id === "disk")?.sections).toEqual([{ id: "history", visible: false }]);
    expect(next.window.tabs.find((t) => t.id === "cpu")?.sections).toEqual([]);
    expect(fallbackSettings.window.tabs.find((t) => t.id === "disk")?.sections).toEqual([]);
  });
});

describe("sectionLabelKey", () => {
  it("names every section of every tab", () => {
    for (const [tab, ids] of Object.entries(TAB_SECTIONS)) {
      for (const id of ids) {
        const key = sectionLabelKey(tab, id);
        expect(key, `${tab}.${id}`).not.toBeNull();
        expect(key! in dict).toBe(true);
      }
    }
  });

  it("is null for a section without a name", () => {
    expect(sectionLabelKey("cpu", "nonsense")).toBeNull();
  });
});
