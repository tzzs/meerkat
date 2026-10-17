import { describe, expect, it } from "vitest";
import type { MetricKind } from "./types";
import {
  MAX_FIGURES,
  SHARE_CARD,
  shareCardSize,
  shareFigures,
  shareLayout,
  type ShareCardInput,
} from "./share";

const figure = (metric: MetricKind, text: string, ratio: number | null = 0.5) => ({
  metric,
  label: metric,
  text,
  ratio,
});

const input = (over: Partial<ShareCardInput> = {}): ShareCardInput => ({
  title: "Meerkat",
  subtitle: "up 3 days",
  figures: [figure("cpu", "42%"), figure("memory", "62%"), figure("disk", "1.4 MB/s"), figure("network", "320 KB/s")],
  ...over,
});

describe("shareCardSize", () => {
  it("is the Open Graph size a link preview crops to", () => {
    expect(SHARE_CARD).toEqual({ width: 1200, height: 630 });
  });

  it("keeps the ratio at another width", () => {
    expect(shareCardSize(600)).toEqual({ width: 600, height: 315 });
  });
});

describe("shareFigures", () => {
  it("keeps the order the window listed them in", () => {
    expect(shareFigures(input()).map((f) => f.metric)).toEqual(["cpu", "memory", "disk", "network"]);
  });

  // A fifth figure is not "one more": the card is a summary, and a table is the window's job.
  it("caps at four", () => {
    const many = input({
      figures: (["cpu", "memory", "disk", "network", "gpu", "battery", "temperature"] as MetricKind[]).map((metric) =>
        figure(metric, "1"),
      ),
    });
    expect(shareFigures(many)).toHaveLength(MAX_FIGURES);
  });

  // The whole point of the card is readings. A figure the machine never gave is a lie in the one
  // place no footnote can explain it, so it is left out rather than drawn as zero.
  it("drops a figure with nothing to show", () => {
    const rows = shareFigures(
      input({ figures: [figure("cpu", "42%"), figure("memory", ""), figure("disk", "1.4 MB/s")] }),
    );
    expect(rows.map((f) => f.metric)).toEqual(["cpu", "disk"]);
  });

  it("drops a figure whose ratio is not a number, and keeps one that has none", () => {
    const rows = shareFigures(
      input({ figures: [figure("cpu", "42%", Number.NaN), figure("gpu", "0%", null)] }),
    );
    expect(rows.map((f) => f.metric)).toEqual(["gpu"]);
  });
});

describe("shareLayout", () => {
  it("fits the columns inside the card with a margin either side", () => {
    const boxes = shareLayout(4);
    expect(boxes).toHaveLength(4);
    expect(boxes[0].x).toBe(64);
    const last = boxes[boxes.length - 1];
    expect(last.x + last.width).toBeLessThanOrEqual(SHARE_CARD.width - 64);
  });

  it("widens the columns when there are fewer, so two figures do not leave a half-empty card", () => {
    const two = shareLayout(2);
    const four = shareLayout(4);
    expect(two[0].width).toBeGreaterThan(four[0].width);
  });

  it("draws nothing for no figures", () => {
    expect(shareLayout(0)).toEqual([]);
  });

  it("keeps the gaps even between neighbours", () => {
    const boxes = shareLayout(3);
    const gaps = boxes.slice(1).map((b, i) => b.x - (boxes[i].x + boxes[i].width));
    expect(new Set(gaps.map((g) => g.toFixed(3))).size).toBe(1);
  });
});
