/**
 * What a settings number box accepts.
 *
 * The box used to commit `Number(text)` on blur, and an emptied field is `Number("")`, which is
 * 0: the CPU threshold became 0%, and every application then counted as misbehaving. A value
 * is now either valid for its field or not saved at all.
 */
export interface NumberRule {
  min: number;
  max: number;
  /** What the arrow keys add or take away. It is a nudge, not a constraint on what may be typed. */
  step: number;
  /** Whole numbers only. */
  integer?: boolean;
}

export type NumberProblem = "empty" | "notNumber" | "tooLow" | "tooHigh" | "notWhole";

export type ParsedNumber = { ok: true; value: number } | { ok: false; problem: NumberProblem };

const DECIMAL = /^[+-]?(\d+\.?\d*|\.\d+)$/;

/**
 * The number a text says, or null if it does not say one. Strictly decimal: `Number` would
 * also take "1e3", "0x10" and "Infinity", none of which anyone typing a threshold means.
 * Full-width digits, which an input method in Chinese mode produces, are folded to ASCII.
 */
export function readNumber(text: string): number | null {
  const folded = text.normalize("NFKC").trim();
  if (!DECIMAL.test(folded)) return null;
  const value = Number(folded);
  return Number.isFinite(value) ? value : null;
}

export function parseNumberInput(text: string, rule: NumberRule): ParsedNumber {
  if (text.trim() === "") return { ok: false, problem: "empty" };
  const value = readNumber(text);
  if (value === null) return { ok: false, problem: "notNumber" };
  if (rule.integer && !Number.isInteger(value)) return { ok: false, problem: "notWhole" };
  if (value < rule.min) return { ok: false, problem: "tooLow" };
  if (value > rule.max) return { ok: false, problem: "tooHigh" };
  return { ok: true, value };
}

/**
 * The text after an arrow key: one step from what is typed, kept inside the rule. Text that is
 * not a number starts from the lowest allowed value rather than from nothing.
 */
export function nudge(text: string, direction: 1 | -1, rule: NumberRule): string {
  const current = readNumber(text) ?? (direction === 1 ? rule.min - rule.step : rule.min);
  const next = Math.min(rule.max, Math.max(rule.min, current + direction * rule.step));
  return String(rule.integer ? Math.round(next) : next);
}
