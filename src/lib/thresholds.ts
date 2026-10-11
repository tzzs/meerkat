import type { NumberRule } from "./number-input";
import type { AlertThresholds, Settings } from "./types";

/** The alert thresholds that are a number in a box: everything but the on/off switch. */
export type AlertField = Exclude<keyof AlertThresholds, "enabled">;

/** How one threshold is shown and entered. */
export interface AlertFieldSpec {
  /** Written after the box. */
  suffix: string;
  /** How many stored units (bytes, bytes per second) one shown unit is; 1 for % and seconds. */
  perUnit: number;
  /** What may be typed, in shown units. */
  rule: NumberRule;
}

const MEGA = 1_000_000;
const MEBI = 1_048_576;

/**
 * The box for one threshold, in the units the rest of the window uses.
 *
 * The alert rows print their limit through the user's storage and network units, so a box that
 * always meant 1 048 576 bytes made "100 MB" typed here read "105 MB" in the list. The size
 * follows the same preference, and a network rate follows bits against bytes.
 *
 * The ranges are the ones the backend would clamp to anyway; refusing them in the box says so
 * at the moment of typing, not after the value has quietly moved.
 */
export function alertFieldSpec(
  field: AlertField,
  prefs: Pick<Settings, "storageUnit" | "networkUnit">,
): AlertFieldSpec {
  const binary = prefs.storageUnit === "binary";
  const size = binary ? MEBI : MEGA;
  switch (field) {
    case "cpuPercent":
      return { suffix: "%", perUnit: 1, rule: { min: 1, max: 100, step: 1, integer: true } };
    case "sustainedSeconds":
      return { suffix: "s", perUnit: 1, rule: { min: 5, max: 3600, step: 5, integer: true } };
    case "memoryGrowBytes":
      return {
        suffix: binary ? "MiB" : "MB",
        perUnit: size,
        rule: { min: 100, max: 65536, step: 100, integer: true },
      };
    case "diskBytesPerSecond":
      return {
        suffix: binary ? "MiB/s" : "MB/s",
        perUnit: size,
        rule: { min: 1, max: 10000, step: 1, integer: true },
      };
    case "networkBytesPerSecond":
      return prefs.networkUnit === "bits"
        ? { suffix: "Mbit/s", perUnit: 125_000, rule: { min: 1, max: 10000, step: 1, integer: true } }
        : {
            suffix: binary ? "MiB/s" : "MB/s",
            perUnit: size,
            rule: { min: 1, max: 10000, step: 1, integer: true },
          };
  }
}

/** The stored number as the box shows it. */
export function toShown(stored: number, spec: AlertFieldSpec): number {
  return Math.round(stored / spec.perUnit);
}

/** What the box says, as the number to store. */
export function toStored(shown: number, spec: AlertFieldSpec): number {
  return Math.round(shown * spec.perUnit);
}

/**
 * How many logical cores a whole-machine percentage amounts to: 25% of a 20-thread chip is
 * five of them. Null when the core count is not known yet.
 */
export function coresForPercent(percent: number, logicalCores: number | null | undefined): number | null {
  if (!logicalCores || logicalCores < 1 || !Number.isFinite(percent)) return null;
  return (Math.max(0, percent) / 100) * logicalCores;
}
