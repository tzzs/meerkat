import type { Settings } from "./types";

const BINARY = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
const DECIMAL = ["B", "KB", "MB", "GB", "TB", "PB"];

export interface UnitPrefs {
  storage: Settings["storageUnit"];
  network: Settings["networkUnit"];
  temperature: Settings["temperatureUnit"];
  cpuScale: Settings["cpuScale"];
  locale: string;
}

export const defaultUnits: UnitPrefs = {
  storage: "decimal",
  network: "bytes",
  temperature: "celsius",
  cpuScale: "perMachine",
  locale: "en-US",
};

function nf(locale: string, digits: number) {
  return new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

function scaleFor(magnitude: number, scale: number, labels: string[]): { index: number; divisor: number } {
  let index = 0;
  let v = magnitude;
  while (v >= scale && index < labels.length - 1) {
    v /= scale;
    index++;
  }
  return { index, divisor: scale ** index };
}

function digits(v: number): number {
  return v >= 100 ? 0 : v >= 10 ? 1 : 2;
}

export function formatBytes(
  bytes: number,
  units: UnitPrefs,
  opts?: { digits?: number; unitOnly?: boolean },
): string {
  const scale = units.storage === "binary" ? 1024 : 1000;
  const labels = units.storage === "binary" ? BINARY : DECIMAL;
  const { index, divisor } = scaleFor(Math.abs(bytes), scale, labels);
  const v = bytes / divisor;
  const digits_ = opts?.digits ?? digits(v);
  const num = nf(units.locale, digits_).format(v);
  return `${num}\u00a0${labels[index]}`;
}

/**
 * A link speed, in the abbreviations Windows itself uses ("1 Gbps", "2.5 Gbps"). Kept apart
 * from `formatRate` because a capacity is a round marketing number while throughput is a
 * measurement, and the two are read differently.
 */
export function formatLinkSpeed(bitsPerSecond: number, locale: string): string {
  const labels = ["bps", "Kbps", "Mbps", "Gbps", "Tbps"];
  const { index, divisor } = scaleFor(bitsPerSecond, 1000, labels);
  const v = bitsPerSecond / divisor;
  // Link speeds are round numbers a person recognises ("1 Gbps", "2.5 Gbps"), so trailing
  // zeros would only add noise.
  const num = new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(v);
  return `${num}\u00a0${labels[index]}`;
}

export function formatRate(
  bytesPerSecond: number,
  units: UnitPrefs,
  kind: "storage" | "network",
): string {
  if (kind === "network" && units.network === "bits") {
    const labels = ["bit/s", "kbit/s", "Mbit/s", "Gbit/s"];
    const { index, divisor } = scaleFor(Math.abs(bytesPerSecond) * 8, 1000, labels);
    const v = (bytesPerSecond * 8) / divisor;
    return `${nf(units.locale, digits(v)).format(v)}\u00a0${labels[index]}`;
  }
  return `${formatBytes(bytesPerSecond, units)}/s`;
}

/**
 * A rate short enough for a 140px tile: "1.2M", "380K". Storage rates count in the unit the
 * user chose (1000 or 1024); only network rates switch to bits. The panel's own version used
 * to multiply disk rates by eight whenever the network setting said bits.
 */
export function formatRateShort(bytesPerSecond: number, units: UnitPrefs, kind: "storage" | "network"): string {
  const bits = kind === "network" && units.network === "bits";
  const value = bits ? bytesPerSecond * 8 : bytesPerSecond;
  const step = bits || units.storage === "decimal" ? 1000 : 1024;
  const labels = bits ? ["b", "Kb", "Mb", "Gb", "Tb"] : ["B", "K", "M", "G", "T"];
  const { index, divisor } = scaleFor(Math.abs(value), step, labels);
  const scaled = value / divisor;
  return `${nf(units.locale, Math.abs(scaled) >= 100 ? 0 : 1).format(scaled)}${labels[index]}`;
}

/**
 * The processor's name as a person would say it. Windows reports the registered marks inline
 * ("Intel(R) Core(TM) Ultra 7 265K"), and the panel used to make room for them by keeping only
 * the first three words — which dropped the model number, the one part that tells chips apart.
 */
export function cleanChipName(name: string): string {
  return name
    .replace(/\((?:R|TM|C)\)|[®™©]/gi, "")
    .replace(/\s+/g, " ")
    .trim();
}

export function formatCompact(n: number, locale: string, digits = 1): string {
  return new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: digits }).format(n);
}

export function formatPercent(value: number, locale = "en-US", digits = 0): string {
  return `${nf(locale, digits).format(value)}%`;
}

export function formatTemp(celsius: number, units: UnitPrefs, withUnit = true): string {
  const f = units.temperature === "fahrenheit";
  const v = f ? celsius * 1.8 + 32 : celsius;
  const text = nf(units.locale, v >= 100 ? 0 : 0).format(v);
  return withUnit ? `${text}\u00b0${f ? "F" : "C"}` : text;
}

/**
 * "1h 2m" or "1 小时 2 分": the two largest units that are not zero. The short form is for
 * places with little room (the status bar, "held for", a battery's remaining time) and also
 * leaves out a part that is exactly zero, so twenty minutes is "20m" rather than "20m 0s".
 * It used to be written in English letters in every locale, which put "3d 5h" in a Chinese
 * window; both forms now use the language's own units.
 */
export function formatDuration(seconds: number, locale: string, opts?: { short?: boolean }): string {
  const s = Math.max(0, Math.round(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = s % 60;
  const short = opts?.short === true;
  const zh = locale.startsWith("zh");
  if (d > 0) {
    if (short && h === 0) return zh ? `${d} 天` : `${d}d`;
    return zh ? `${d} 天 ${h} 小时` : `${d}d ${h}h`;
  }
  if (h > 0) {
    if (short && m === 0) return zh ? `${h} 小时` : `${h}h`;
    return zh ? `${h} 小时 ${m} 分` : `${h}h ${m}m`;
  }
  if (m > 0) {
    if (short && rest === 0) return zh ? `${m} 分钟` : `${m}m`;
    return zh ? `${m} 分 ${rest} 秒` : `${m}m ${rest}s`;
  }
  return zh ? `${s} 秒` : `${s}s`;
}

export function formatClock(secondsOfDay: number | null): string {
  if (secondsOfDay === null || !isFinite(secondsOfDay) || secondsOfDay < 0) return "—";
  const h = Math.floor(secondsOfDay / 3600);
  const m = Math.floor((secondsOfDay % 3600) / 60);
  return `${h}:${String(m).padStart(2, "0")}`;
}

export function formatTime(atMs: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }).format(atMs);
}

export function formatDay(atMs: number, locale: string, timeZone?: string): string {
  return new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", weekday: "short", timeZone }).format(atMs);
}

/** CPU percent is either 0–100 across the machine or 0–100 per core. */
export function cpuDisplay(processPercent: number, units: UnitPrefs): number {
  return units.cpuScale === "perCore" ? processPercent : processPercent / Math.max(1, countLogical());
}

/**
 * A share of the whole machine, which is how the backend measures alert thresholds and hourly
 * rankings (100% = every core busy), in the user's CPU unit: as it is, or - for someone who
 * reads CPU per core (100% = one core busy) - that many times larger. Null for a figure that is
 * not a number, so the caller can print a dash instead of a zero nobody measured; a negative
 * share, which an old record can carry, is held at zero.
 */
export function machineShareDisplay(percent: number, units: UnitPrefs): number | null {
  if (!Number.isFinite(percent)) return null;
  const share = Math.max(0, percent);
  return units.cpuScale === "perCore" ? share * Math.max(1, countLogical()) : share;
}

let cachedCores: number | null = null;
function countLogical(): number {
  if (cachedCores === null) cachedCores = navigator.hardwareConcurrency || 1;
  return cachedCores;
}

export function setCores(n: number) {
  cachedCores = n;
}

export function bytesPerSecondToMbps(bytesPerSecond: number): number {
  return (bytesPerSecond * 8) / 1_000_000;
}
