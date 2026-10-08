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

export function formatDuration(seconds: number, locale: string, opts?: { short?: boolean }): string {
  const s = Math.max(0, Math.round(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const zh = locale.startsWith("zh");
  if (d > 0) return opts?.short ? `${d}${h ? ` ${h}h` : ""}` : zh ? `${d} 天 ${h} 小时` : `${d}d ${h}h`;
  if (h > 0) return opts?.short ? `${h}h ${m}m` : zh ? `${h} 小时 ${m} 分` : `${h}h ${m}m`;
  if (m > 0) return zh ? `${m} 分 ${s % 60} 秒` : `${m}m ${s % 60}s`;
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

export function formatDay(atMs: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", weekday: "short" }).format(atMs);
}

/** CPU percent is either 0–100 across the machine or 0–100 per core. */
export function cpuDisplay(processPercent: number, units: UnitPrefs): number {
  return units.cpuScale === "perCore" ? processPercent : processPercent / Math.max(1, countLogical());
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
