import { describe, expect, it } from "vitest";
import { parseNumberInput } from "./number-input";
import { alertFieldSpec, coresForPercent, toShown, toStored } from "./thresholds";

const decimal = { storageUnit: "decimal", networkUnit: "bytes" } as const;
const binary = { storageUnit: "binary", networkUnit: "bytes" } as const;
const bits = { storageUnit: "decimal", networkUnit: "bits" } as const;

describe("alertFieldSpec", () => {
  it("has the ranges the thresholds are held to", () => {
    expect(alertFieldSpec("cpuPercent", decimal).rule).toMatchObject({ min: 1, max: 100, integer: true });
    expect(alertFieldSpec("sustainedSeconds", decimal).rule).toMatchObject({ min: 5, max: 3600 });
    expect(alertFieldSpec("memoryGrowBytes", decimal).rule).toMatchObject({ min: 100, max: 65536 });
    expect(alertFieldSpec("diskBytesPerSecond", decimal).rule).toMatchObject({ min: 1, max: 10000 });
    expect(alertFieldSpec("networkBytesPerSecond", decimal).rule).toMatchObject({ min: 1, max: 10000 });
  });

  it("counts in the units the rest of the window prints", () => {
    expect(alertFieldSpec("memoryGrowBytes", decimal).suffix).toBe("MB");
    expect(alertFieldSpec("memoryGrowBytes", binary).suffix).toBe("MiB");
    expect(alertFieldSpec("diskBytesPerSecond", binary).suffix).toBe("MiB/s");
    expect(alertFieldSpec("networkBytesPerSecond", decimal).suffix).toBe("MB/s");
    expect(alertFieldSpec("networkBytesPerSecond", bits).suffix).toBe("Mbit/s");
  });

  it("refuses zero for every threshold", () => {
    for (const field of [
      "cpuPercent",
      "sustainedSeconds",
      "memoryGrowBytes",
      "diskBytesPerSecond",
      "networkBytesPerSecond",
    ] as const) {
      expect(parseNumberInput("0", alertFieldSpec(field, decimal).rule).ok).toBe(false);
    }
  });
});

describe("toShown and toStored", () => {
  it("round-trip the defaults without moving them", () => {
    const disk = alertFieldSpec("diskBytesPerSecond", decimal);
    expect(toShown(40_000_000, disk)).toBe(40);
    expect(toStored(40, disk)).toBe(40_000_000);
    const network = alertFieldSpec("networkBytesPerSecond", bits);
    expect(toShown(20_000_000, network)).toBe(160);
    expect(toStored(160, network)).toBe(20_000_000);
  });

  // 100 typed in a decimal window must come back as 100 MB in the alert list, not as 105.
  it("stores what is typed in the unit it will be printed in", () => {
    expect(toStored(100, alertFieldSpec("memoryGrowBytes", decimal))).toBe(100_000_000);
    expect(toStored(100, alertFieldSpec("memoryGrowBytes", binary))).toBe(104_857_600);
  });

  it("leaves a percentage and a duration alone", () => {
    expect(toStored(25, alertFieldSpec("cpuPercent", decimal))).toBe(25);
    expect(toShown(30, alertFieldSpec("sustainedSeconds", binary))).toBe(30);
  });
});

describe("coresForPercent", () => {
  it("is the share of the machine's logical cores", () => {
    expect(coresForPercent(25, 20)).toBe(5);
    expect(coresForPercent(100, 8)).toBe(8);
    expect(coresForPercent(3, 20)).toBeCloseTo(0.6);
  });

  it("is unknown rather than zero before the core count is", () => {
    expect(coresForPercent(25, null)).toBeNull();
    expect(coresForPercent(25, undefined)).toBeNull();
    expect(coresForPercent(25, 0)).toBeNull();
    expect(coresForPercent(Number.NaN, 8)).toBeNull();
  });
});
