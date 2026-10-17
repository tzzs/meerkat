import { describe, expect, it } from "vitest";
import {
  cleanChipName,
  cpuDisplay,
  defaultUnits,
  formatBytes,
  formatClock,
  formatDuration,
  formatLinkSpeed,
  formatPercent,
  formatRate,
  formatRateShort,
  formatTemp,
  machineShareDisplay,
  setCores,
  type UnitPrefs,
} from "./format";

const NBSP = " ";

const units = (over: Partial<UnitPrefs> = {}): UnitPrefs => ({ ...defaultUnits, ...over });

describe("formatBytes", () => {
  it("separates the number from the unit with a non-breaking space", () => {
    expect(formatBytes(21_300_000_000, units())).toBe(`21.3${NBSP}GB`);
  });

  it("counts thousands as decimal and 1024s as binary", () => {
    expect(formatBytes(1000, units())).toBe(`1.00${NBSP}KB`);
    expect(formatBytes(1024, units({ storage: "binary" }))).toBe(`1.00${NBSP}KiB`);
    expect(formatBytes(1024 * 1024, units({ storage: "binary" }))).toBe(`1.00${NBSP}MiB`);
  });

  it("keeps three significant figures rather than three decimals", () => {
    expect(formatBytes(999, units())).toBe(`999${NBSP}B`);
    expect(formatBytes(10_000_000, units())).toBe(`10.0${NBSP}MB`);
    expect(formatBytes(1_000_000_000, units())).toBe(`1.00${NBSP}GB`);
  });
});

describe("formatRate", () => {
  // The unit is chosen by the value itself. An earlier attempt to "stabilise" the display by
  // pinning it to a reference figure made a 10 KB/s rate read 0.01 MB/s, which is a worse lie
  // than a changing unit.
  it("does not borrow a larger unit from a nearby figure", () => {
    expect(formatRate(10_240, units(), "storage")).toBe(`10.2${NBSP}KB/s`);
    expect(formatRate(0.5, units(), "storage")).toBe(`0.50${NBSP}B/s`);
  });

  it("multiplies by eight and switches to bit labels for network rates", () => {
    expect(formatRate(1_000_000, units({ network: "bits" }), "network")).toBe(`8.00${NBSP}Mbit/s`);
    expect(formatRate(1_000_000, units({ network: "bytes" }), "network")).toBe(`1.00${NBSP}MB/s`);
  });
});

describe("formatRateShort", () => {
  it("fits a tile and counts in the storage unit the user chose", () => {
    expect(formatRateShort(1_200_000, units(), "storage")).toBe("1.2M");
    expect(formatRateShort(1_258_291, units({ storage: "binary" }), "storage")).toBe("1.2M");
    expect(formatRateShort(380_000, units(), "storage")).toBe("380K");
    expect(formatRateShort(0, units(), "storage")).toBe("0.0B");
  });

  // The panel's old helper read the network setting for every rate, so choosing bits for the
  // network made the disk tile report its bytes multiplied by eight.
  it("only switches network rates to bits", () => {
    expect(formatRateShort(1_000_000, units({ network: "bits" }), "network")).toBe("8.0Mb");
    expect(formatRateShort(1_000_000, units({ network: "bits" }), "storage")).toBe("1.0M");
  });
});

describe("cleanChipName", () => {
  it("drops the registered marks and keeps the model number", () => {
    expect(cleanChipName("Intel(R) Core(TM) Ultra 7 265K")).toBe("Intel Core Ultra 7 265K");
    expect(cleanChipName("Intel® Core™ i5-8250U CPU @ 1.60GHz")).toBe("Intel Core i5-8250U CPU @ 1.60GHz");
  });

  it("leaves a name without marks alone", () => {
    expect(cleanChipName("AMD Ryzen 9 7950X 16-Core Processor ")).toBe("AMD Ryzen 9 7950X 16-Core Processor");
  });
});

describe("formatLinkSpeed", () => {
  it("writes capacities the way Windows labels them", () => {
    expect(formatLinkSpeed(1_000_000_000, "en-US")).toBe(`1${NBSP}Gbps`);
    expect(formatLinkSpeed(2_500_000_000, "en-US")).toBe(`2.5${NBSP}Gbps`);
    expect(formatLinkSpeed(1_000_000_000_000, "en-US")).toBe(`1${NBSP}Tbps`);
  });

  it("carries no fraction for a zero link rather than reporting 0 KB/s", () => {
    expect(formatLinkSpeed(0, "en-US")).toBe(`0${NBSP}bps`);
  });

  it("rounds a measured speed to two decimals at most", () => {
    expect(formatLinkSpeed(1_234_567, "en-US")).toBe(`1.23${NBSP}Mbps`);
  });
});

describe("formatTemp", () => {
  it("converts to Fahrenheit and back", () => {
    expect(formatTemp(37, units())).toBe("37°C");
    expect(formatTemp(0, units({ temperature: "fahrenheit" }))).toBe("32°F");
    expect(formatTemp(37, units({ temperature: "fahrenheit" }))).toBe("99°F");
  });

  it("can leave the unit off for a label that carries it", () => {
    expect(formatTemp(37, units({ temperature: "fahrenheit" }), false)).toBe("99");
  });
});

describe("formatDuration", () => {
  it("speaks the locale it is given", () => {
    expect(formatDuration(3725, "en-US")).toBe("1h 2m");
    expect(formatDuration(3725, "zh-CN")).toBe("1 小时 2 分");
    expect(formatDuration(90061, "zh-CN")).toBe("1 天 1 小时");
  });

  it("has a short form for the status bar", () => {
    // The day unit used to be dropped here, so a machine three days old read "3 5h".
    expect(formatDuration(90061, "en-US", { short: true })).toBe("1d 1h");
    expect(formatDuration(87000, "en-US", { short: true })).toBe("1d");
    expect(formatDuration(3725, "en-US", { short: true })).toBe("1h 2m");
  });

  it("never counts backwards", () => {
    expect(formatDuration(-5, "en-US")).toBe("0s");
    expect(formatDuration(-5, "zh-CN")).toBe("0 秒");
  });

  // The short form was English letters in every locale, so a Chinese status bar read "3d 5h".
  it("writes the short form in the language's own units", () => {
    expect(formatDuration(90061, "zh-CN", { short: true })).toBe("1 天 1 小时");
    expect(formatDuration(3725, "zh-CN", { short: true })).toBe("1 小时 2 分");
    expect(formatDuration(125, "zh-CN", { short: true })).toBe("2 分 5 秒");
    expect(formatDuration(45, "zh-CN", { short: true })).toBe("45 秒");
    for (const seconds of [45, 125, 3725, 90061]) {
      expect(formatDuration(seconds, "zh-CN", { short: true })).not.toMatch(/[dhms]\b/);
    }
  });

  it("leaves out a part that is exactly zero in the short form only", () => {
    expect(formatDuration(1200, "en-US", { short: true })).toBe("20m");
    expect(formatDuration(1200, "zh-CN", { short: true })).toBe("20 分钟");
    expect(formatDuration(7200, "en-US", { short: true })).toBe("2h");
    expect(formatDuration(7200, "zh-CN", { short: true })).toBe("2 小时");
    expect(formatDuration(87000, "zh-CN", { short: true })).toBe("1 天");
    // The long form keeps both parts, as the battery's remaining time always has.
    expect(formatDuration(1200, "en-US")).toBe("20m 0s");
    expect(formatDuration(87000, "zh-CN")).toBe("1 天 0 小时");
  });

  it("rounds to whole seconds before it splits", () => {
    expect(formatDuration(59.6, "en-US")).toBe("1m 0s");
    expect(formatDuration(0.4, "en-US")).toBe("0s");
  });
});

describe("formatClock", () => {
  it("shows an em dash for a sleep that never happened", () => {
    expect(formatClock(null)).toBe("—");
    expect(formatClock(-1)).toBe("—");
    expect(formatClock(Number.NaN)).toBe("—");
  });

  it("pads the minutes but not the hours", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(3725)).toBe("1:02");
    expect(formatClock(9 * 3600)).toBe("9:00");
  });
});

describe("cpuDisplay", () => {
  it("divides by the core count only when the whole machine is the scale", () => {
    setCores(20);
    expect(cpuDisplay(100, units())).toBe(5);
    expect(cpuDisplay(100, units({ cpuScale: "perCore" }))).toBe(100);
  });
});

describe("machineShareDisplay", () => {
  it("leaves a share of the whole machine alone when that is the scale", () => {
    setCores(20);
    expect(machineShareDisplay(25, units())).toBe(25);
  });

  // 25% of a twenty-thread machine is five cores busy, which is 500% when 100% is one core.
  it("multiplies by the core count when the scale is per core", () => {
    setCores(20);
    expect(machineShareDisplay(25, units({ cpuScale: "perCore" }))).toBe(500);
  });

  it("holds a negative share at zero and does not turn garbage into a number", () => {
    setCores(8);
    expect(machineShareDisplay(-4, units())).toBe(0);
    expect(machineShareDisplay(-4, units({ cpuScale: "perCore" }))).toBe(0);
    expect(machineShareDisplay(Number.NaN, units())).toBeNull();
    expect(machineShareDisplay(Number.POSITIVE_INFINITY, units())).toBeNull();
  });

  it("lets an old record above 100 through rather than inventing a correction", () => {
    setCores(8);
    expect(machineShareDisplay(388, units())).toBe(388);
  });
});

describe("formatPercent", () => {
  it("adds the decimals the figure needs", () => {
    expect(formatPercent(17, "en-US")).toBe("17%");
    expect(formatPercent(2.34, "en-US", 1)).toBe("2.3%");
  });
});
