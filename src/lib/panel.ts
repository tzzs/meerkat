import { sumSeries } from "./spark";
import { isMetricKind, powerTabMode, seriesFor, type MetricKind, type SectionConfig, type Snapshot } from "./types";

/** The tray panel's own tabs: the busiest-apps view, and one per metric. */
export type PanelTab = MetricKind | "top";

/**
 * The panel's tab strip, in order. The power slot follows the same rule as the window's power
 * tab: battery on a laptop, temperature on a desktop that reports one, nothing otherwise.
 */
export function panelNav(snapshot: Snapshot | null): PanelTab[] {
  const nav: PanelTab[] = ["top", "cpu", "memory", "disk", "network", "gpu"];
  const mode = powerTabMode(snapshot);
  if (mode === "battery") nav.push("battery");
  else if (mode === "thermal") nav.push("temperature");
  return nav;
}

/**
 * A digit picks the tab at that position in the strip on screen. The map used to be its own
 * list, which had a temperature entry the strip never showed — so "8" selected a tab that had
 * no button and no way back to it except another digit.
 */
export function panelTabForKey(key: string, nav: readonly PanelTab[]): PanelTab | null {
  if (!/^[1-9]$/.test(key)) return null;
  return nav[Number(key) - 1] ?? null;
}

/**
 * The tiles to draw, in the saved order: shown, drawable, and the battery only on a machine
 * that has one — a desktop's battery tile was a dash under a label that promised a reading.
 */
export function panelTiles(tiles: readonly SectionConfig[], snapshot: Snapshot): MetricKind[] {
  return tiles
    .filter((tile) => tile.visible)
    .map((tile) => tile.id)
    .filter(isMetricKind)
    .filter((metric) => metric !== "battery" || snapshot.battery.hasBattery);
}

/**
 * The history a tile draws. Disk and network tiles print both directions, so their line is the
 * two added together; it used to follow reads and downloads only, and a write burst that the
 * figure above it reported left the line flat.
 */
export function panelSeries(metric: MetricKind, snapshot: Snapshot): number[] {
  if (metric === "disk") return sumSeries(snapshot.disk.historyRead, snapshot.disk.historyWrite);
  if (metric === "network") return sumSeries(snapshot.network.historyDownload, snapshot.network.historyUpload);
  return seriesFor(metric, snapshot);
}

/** What the panel card measures, in CSS pixels. */
export interface PanelBox {
  scrollWidth: number;
  scrollHeight: number;
  /** Border (and any scrollbar) on each axis: offset size minus client size. */
  borderX: number;
  borderY: number;
  /** The card's own margins, which leave room for the shadow a transparent window lacks. */
  marginX: number;
  marginY: number;
}

export const PANEL_MIN_WIDTH = 320;
export const PANEL_MIN_HEIGHT = 220;
export const PANEL_MAX_HEIGHT = 760;

/**
 * The card's natural height, before a window that is too short has squeezed its flex children.
 * `.panel.scrollHeight` is no good for sizing the window: once the card has been clipped it
 * reports the clipped height, so the window is asked to stay the size that caused the clipping.
 * Measuring each section's own scroll height gives the size the card actually wants.
 */
export function panelContentHeight(sections: readonly number[]): number {
  return sections.reduce((total, height) => total + (Number.isFinite(height) ? Math.max(0, height) : 0), 0);
}

/**
 * The window size that holds the card, its border and the margin around it. Sized to the
 * card's scroll box alone, the window came out one border and one margin short on each axis,
 * which cut the strip of tabs and the shadow off the bottom edge.
 */
export function panelWindowSize(box: PanelBox): { width: number; height: number } {
  const width = Math.ceil(box.scrollWidth + box.borderX + box.marginX);
  const height = Math.ceil(box.scrollHeight + box.borderY + box.marginY);
  return {
    width: Math.max(PANEL_MIN_WIDTH, width),
    height: Math.min(PANEL_MAX_HEIGHT, Math.max(PANEL_MIN_HEIGHT, height)),
  };
}

/**
 * How many app rows fit in the room the panel leaves the list. The fit is floored, so a
 * half-visible row is dropped whole instead of clipped, and it never falls below the minimum
 * while the list has that many apps — a first paint that has measured nothing opens with rows
 * rather than an empty card.
 */
export function fitRows(room: number, rowHeight: number, count: number, minimum: number): number {
  const floor = Math.min(minimum, count);
  if (!Number.isFinite(room) || !Number.isFinite(rowHeight) || room <= 0 || rowHeight <= 0) return floor;
  return Math.max(floor, Math.min(count, Math.floor(room / rowHeight)));
}
