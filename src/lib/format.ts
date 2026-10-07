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

export function formatBytes(bytes: number, units: UnitPrefs, opts?: { digits?: number; unitOnly?: boolean }): string {
  const scale = units.storage === "binary" ? 1024 : 1000;
  const labels = units.storage === "binary" ? BINARY : DECIMAL;
  const abs = Math.abs(bytes);
  let i = 0;
  let v = abs;
  while (v >= scale && i < labels.length - 1) {
    v /= scale;
    i++;
  }
  const digits = opts?.digits ?? (v >= 100 ? 0 : v >= 10 ? 1 : 2);
  const num = nf(units.locale, digits).format(v);
  return `${num}\u00a0${labels[i]}`;
}

export function formatRate(bytesPerSecond: number, units: UnitPrefs, kind: "storage" | "network"): string {
  if (kind === "network" && units.network === "bits") {
    const bits = bytesPerSecond * 8;
    const scale = 1000;
    const labels = ["bit/s", "kbit/s", "Mbit/s", "Gbit/s"];
    let i = 0;
    let v = Math.abs(bits);
    while (v >= scale && i < labels.length - 1) {
      v /= scale;
      i++;
    }
    const digits = v >= 100 ? 0 : v >= 10 ? 1 : 2;
    return `${nf(units.locale, digits).format(v)}\u00a0${labels[i]}`;
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
