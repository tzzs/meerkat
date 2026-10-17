import type { CapacityPoint, DailyTotal } from "./types";

export interface Sums {
  read: number;
  write: number;
  download: number;
  upload: number;
}

const ZERO: Sums = { read: 0, write: 0, download: 0, upload: 0 };

/**
 * The last `days` of `rows` (oldest first, today last) added up. The stored row for today is a
 * minute behind the sampler at worst, so today's live counts replace it when they are given.
 */
export function sumDays(rows: DailyTotal[], days: number, today?: Partial<Sums>): Sums {
  const span = rows.slice(-Math.max(1, days));
  const sums = { ...ZERO };
  span.forEach((row, index) => {
    const isToday = index === span.length - 1 && today !== undefined;
    sums.read += isToday && today.read !== undefined ? today.read : row.readBytes;
    sums.write += isToday && today.write !== undefined ? today.write : row.writeBytes;
    sums.download += isToday && today.download !== undefined ? today.download : row.downloadBytes;
    sums.upload += isToday && today.upload !== undefined ? today.upload : row.uploadBytes;
  });
  if (span.length === 0 && today) return { ...ZERO, ...today };
  return sums;
}

/**
 * How a battery's full charge has moved against its design capacity, first week to last. Null
 * until there are two weeks to compare.
 */
export function capacityTrend(points: CapacityPoint[]): { from: number; to: number; weeks: number } | null {
  const usable = points.filter((p) => p.designWh > 0);
  if (usable.length < 2) return null;
  const first = usable[0];
  const last = usable[usable.length - 1];
  return {
    from: (first.fullChargeWh / first.designWh) * 100,
    to: (last.fullChargeWh / last.designWh) * 100,
    weeks: Math.max(1, Math.round((last.atMs - first.atMs) / (7 * 86_400_000))),
  };
}
