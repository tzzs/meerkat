import { dict, type Key } from "../i18n/en";
import type { SectionConfig, Settings, TabConfig } from "./types";

/**
 * The sections each window tab is made of, in their default order. The cards at the top of a
 * tab (the figure and its meter) are what the tab is, not sections, and cannot be hidden.
 *
 * The settings file stores a list per tab, empty by default, and every id in it has to be one
 * of these: that is what keeps a file written by another version from naming a card this build
 * has never drawn.
 */
export const TAB_SECTIONS: Readonly<Record<string, readonly string[]>> = {
  overview: ["charts", "topApps", "worthALook", "runaway", "storage", "byHour"],
  cpu: ["perCore", "history", "apps"],
  memory: ["breakdown", "history", "apps"],
  disk: ["volumes", "storageMap", "history", "apps"],
  network: ["adapters", "history", "connections"],
  gpu: ["apps", "history"],
  battery: ["power", "history"],
  sound: ["output", "apps", "input"],
};

function defaultOrder(tab: string): readonly string[] {
  return Object.hasOwn(TAB_SECTIONS, tab) ? TAB_SECTIONS[tab] : [];
}

/**
 * A tab's sections as the window draws them, hidden ones included: the saved list in its saved
 * order, then whatever the saved list does not mention, visible and in default order. An empty
 * saved list therefore means "all of them, as designed", and a list written before a section
 * existed shows the new one rather than losing it. Ids this build does not know are dropped.
 */
export function effectiveSections(config: Pick<TabConfig, "sections"> | undefined, tab: string): SectionConfig[] {
  const known = defaultOrder(tab);
  const seen = new Set<string>();
  const out: SectionConfig[] = [];
  for (const section of config?.sections ?? []) {
    if (!known.includes(section.id) || seen.has(section.id)) continue;
    seen.add(section.id);
    out.push({ id: section.id, visible: section.visible });
  }
  for (const id of known) if (!seen.has(id)) out.push({ id, visible: true });
  return out;
}

/**
 * The section ids a tab should draw, in the order to draw them. The tabs used to ask "is this
 * one shown?" of a fixed sequence, so a saved order was stored and then ignored.
 */
export function visibleSections(settings: Pick<Settings, "window">, tab: string): string[] {
  const config = settings.window.tabs.find((t) => t.id === tab);
  return effectiveSections(config, tab)
    .filter((section) => section.visible)
    .map((section) => section.id);
}

/**
 * Whether a card is drawn. `shown` is what [`visibleSections`] returned; a tab whose saved list
 * is empty draws everything, which is why the callers pass the effective list rather than the
 * saved one.
 */
export function sectionShown(shown: readonly string[], id: string): boolean {
  return shown.includes(id);
}

/**
 * `id` one place earlier or later among the items `movable` accepts; the others stay where they
 * are. The panel's tile list can hold ids that are not metrics, which are never drawn and are
 * not offered for reordering, so a step has to skip over them rather than swap with one.
 */
export function moveWithin<T extends { id: string }>(
  list: readonly T[],
  id: string,
  delta: -1 | 1,
  movable: (item: T) => boolean = () => true,
): T[] {
  const out = [...list];
  const from = out.findIndex((item) => item.id === id);
  if (from === -1) return out;
  for (let to = from + delta; to >= 0 && to < out.length; to += delta) {
    if (!movable(out[to])) continue;
    [out[from], out[to]] = [out[to], out[from]];
    return out;
  }
  return out;
}

/** The settings with one tab's section list replaced; an empty list is "as designed". */
export function withTabSections<S extends Pick<Settings, "window">>(
  settings: S,
  tab: string,
  sections: SectionConfig[],
): S {
  return {
    ...settings,
    window: {
      ...settings.window,
      tabs: settings.window.tabs.map((config) => (config.id === tab ? { ...config, sections } : config)),
    },
  };
}

/** The translation key naming a section, or null when this build has no name for it. */
export function sectionLabelKey(tab: string, id: string): Key | null {
  const key = `section.${tab}.${id}`;
  return key in dict ? (key as Key) : null;
}
