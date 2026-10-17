import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "./ui/button";
import { useStore } from "../lib/store";
import { formatDuration, formatPercent, formatRate, type UnitPrefs } from "../lib/format";
import { headline, type MetricKind, type Snapshot } from "../lib/types";
import { SHARE_CARD, shareFigures, shareLayout, type ShareCardInput } from "../lib/share";

/**
 * The share card: a 1200×630 PNG of what the machine is doing now, drawn on a canvas.
 *
 * Every figure on it is a string the window already prints — nothing is computed here — so the
 * card cannot disagree with the screen it was exported from. The drawing also reads the app's
 * own computed colours, for the same reason: a card in a different palette to the window is a
 * second app.
 */
export function ShareCardButton() {
  const { snapshot, units, t } = useStore();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [saved, setSaved] = useState(false);

  const card = useMemo<ShareCardInput>(
    () => ({
      title: t("app.name"),
      subtitle: snapshot
        ? `${snapshot.machine.chipName || t("app.name")} · ${t("machine.up", {
            duration: formatDuration(snapshot.machine.uptimeSeconds, units.locale, { short: true }),
          })}`
        : t("status.waiting"),
      figures: snapshot ? figuresOf(snapshot, units, t) : [],
    }),
    [snapshot, units, t],
  );

  // Repainted with the figures, not on a timer: the card is a still of the moment it was made.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.width = SHARE_CARD.width;
    canvas.height = SHARE_CARD.height;
    const ctx = canvas.getContext("2d");
    if (ctx) drawCard(ctx, card);
  }, [card]);

  const rows = shareFigures(card);

  return (
    <div className="share-card-wrap">
      {/* Drawn at the card's real size and scaled by CSS: the PNG that goes out is full size,
          and what is on screen is a preview of it, not a smaller drawing. */}
      <canvas ref={canvasRef} className="share-card" role="img" aria-label={t("settings.shareAlt")} />
      <Button
        variant="chip"
        disabled={rows.length === 0}
        onClick={() => {
          const canvas = canvasRef.current;
          if (!canvas) return;
          const link = document.createElement("a");
          link.href = canvas.toDataURL("image/png");
          link.download = "meerkat.png";
          link.click();
          setSaved(true);
          window.setTimeout(() => setSaved(false), 1500);
        }}
      >
        {saved ? t("action.done") : t("settings.shareSave")}
      </Button>
    </div>
  );
}

/**
 * The four the card is built from, each worded exactly as the window words it. GPU, battery and
 * temperature are deliberately not candidates: a card of four is a summary, and a fifth figure
 * is the point at which it becomes the table the window already is.
 */
function figuresOf(
  snapshot: Snapshot,
  units: UnitPrefs,
  t: ReturnType<typeof useStore>["t"],
): ShareCardInput["figures"] {
  const wanted: MetricKind[] = ["cpu", "memory", "disk", "network"];
  return wanted.map((metric) => {
    const value = headline(metric, snapshot);
    const percent = metric === "cpu" || metric === "memory";
    return {
      metric,
      label: t(`metric.${metric}`),
      text: percent
        ? formatPercent(value, units.locale)
        : formatRate(value, units, metric === "network" ? "network" : "storage"),
      ratio: percent ? value / 100 : null,
    };
  });
}

/** Reads the app's own colours off the canvas, so the card is the theme it was exported from. */
function token(el: HTMLElement, name: string, fallback: string): string {
  return getComputedStyle(el).getPropertyValue(name).trim() || fallback;
}

function drawCard(ctx: CanvasRenderingContext2D, card: ShareCardInput) {
  const canvas = ctx.canvas;
  const { width, height } = SHARE_CARD;
  const rows = shareFigures(card);
  const boxes = shareLayout(rows.length);
  // The canvas has no wrapping and no ellipsis, so every string that has to stay inside its
  // column goes through this.
  const column = boxes.length > 0 ? boxes[0].width : width;
  const label = (text: string, x: number, y: number) => ctx.fillText(clip(ctx, text, column), x, y);

  ctx.fillStyle = token(canvas, "--bg", "#131315");
  ctx.fillRect(0, 0, width, height);

  ctx.textBaseline = "alphabetic";
  ctx.fillStyle = token(canvas, "--text", "#f5f5f7");
  ctx.font = `700 40px "Segoe UI Variable Display", system-ui, sans-serif`;
  ctx.fillText(card.title, 64, 128);

  ctx.fillStyle = token(canvas, "--text-2", "#a1a1a6");
  ctx.font = `400 22px "Segoe UI Variable Text", system-ui, sans-serif`;
  label(card.subtitle, 64, 172);

  rows.forEach((row, index) => {
    const box = boxes[index];
    const tone = token(canvas, `--m-${row.metric}`, "#3987e5");

    ctx.fillStyle = token(canvas, "--text-2", "#a1a1a6");
    ctx.font = `600 20px "Segoe UI Variable Text", system-ui, sans-serif`;
    label(row.label.toUpperCase(), box.x, 300);

    ctx.fillStyle = tone;
    ctx.font = `700 56px "Segoe UI Variable Display", system-ui, sans-serif`;
    label(row.text, box.x, 380);

    // The tinted track and its fill: the same shape a meter has in the window.
    const trackY = 470;
    const trackH = 12;
    ctx.fillStyle = withAlpha(tone, 0.16);
    roundRect(ctx, box.x, trackY, box.width, trackH, trackH / 2);
    ctx.fill();
    if (row.ratio !== null) {
      ctx.fillStyle = tone;
      const fill = Math.max(trackH, box.width * Math.min(1, Math.max(0, row.ratio)));
      roundRect(ctx, box.x, trackY, fill, trackH, trackH / 2);
      ctx.fill();
    }
  });

  ctx.fillStyle = token(canvas, "--text-3", "#6e6e73");
  ctx.font = `500 18px "Segoe UI Variable Text", system-ui, sans-serif`;
  ctx.fillText(`${width}×${height}`, 64, height - 48);
}

/** A string that will not be drawn past its column; the canvas has no text wrapping. */
function clip(ctx: CanvasRenderingContext2D, text: string, max: number): string {
  if (ctx.measureText(text).width <= max) return text;
  let out = text;
  while (out.length > 1 && ctx.measureText(`${out}…`).width > max) out = out.slice(0, -1);
  return `${out}…`;
}

function withAlpha(color: string, alpha: number): string {
  const hex = /#([0-9a-f]{6})/i.exec(color.trim());
  if (hex) {
    const value = Number.parseInt(hex[1], 16);
    return `rgba(${(value >> 16) & 255}, ${(value >> 8) & 255}, ${value & 255}, ${alpha})`;
  }
  const rgb = /rgba?\(([^)]+)\)/i.exec(color.trim());
  if (rgb) {
    const [r, g, b] = rgb[1].split(",").map((part) => Number.parseFloat(part.trim()));
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }
  return color;
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const radius = Math.min(r, h / 2, w / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}
