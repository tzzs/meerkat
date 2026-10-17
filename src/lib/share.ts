import type { MetricKind } from "./types";

/** The share card's canvas. 1200×630 is the Open Graph size every link preview crops to. */
export const SHARE_CARD = { width: 1200, height: 630 } as const;

/** The same ratio at another width, for a preview thumbnail. */
export function shareCardSize(width: number): { width: number; height: number } {
  return { width, height: Math.round((width * SHARE_CARD.height) / SHARE_CARD.width) };
}

/** One figure the card is asked to draw, already in the words the window uses. */
export interface ShareFigure {
  metric: MetricKind;
  /** The name over the figure. */
  label: string;
  /** What the window prints for it: "42%", "1.4 MB/s". Formatting stays in the app's hands. */
  text: string;
  /** For the bar under the figure: 0…1, or null when there is nothing to scale against. */
  ratio: number | null;
}

export interface ShareCardInput {
  title: string;
  subtitle: string;
  figures: ShareFigure[];
}

/** The card holds four figures. A fifth is not "one more", it is a table. */
export const MAX_FIGURES = 4;

/**
 * The figures the card draws: at most four, in the order they arrived, and without any whose
 * text is empty. A reading Windows could not give is left out rather than drawn as zero — on a
 * card nobody can hover for the footnote, a zero is a statement the machine never made.
 */
export function shareFigures(input: ShareCardInput): ShareFigure[] {
  return input.figures
    .filter((figure) => figure.text.trim() !== "" && Number.isFinite(figure.ratio ?? 0))
    .slice(0, MAX_FIGURES);
}

/** Where each figure sits: a row of equal columns inside the card's own margin. */
export function shareLayout(
  count: number,
  size: { width: number; height: number } = SHARE_CARD,
): { x: number; y: number; width: number }[] {
  if (count <= 0) return [];
  const margin = 64;
  const gap = 32;
  const usable = size.width - margin * 2 - gap * (count - 1);
  const width = usable / count;
  return Array.from({ length: count }, (_, index) => ({
    x: margin + index * (width + gap),
    y: 250,
    width,
  }));
}
