/** One rectangle of a squarified treemap, in a 1×1 box: each edge is a fraction. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * A squarified treemap: `values` as areas, tiled into a 1×1 box. Rectangles are laid out along
 * the shortest side of what remains, and a new row starts when adding the next value would make
 * the row's aspect ratios worse — so a set of roughly equal values draws as a grid rather than a
 * fan of slivers, and every rectangle's area stays proportional to its value.
 *
 * The values need not sum to any particular number: each is scaled by the total, so the whole
 * box is always filled. A value that is not positive draws no rectangle (area 0).
 */
export function squarify(values: readonly number[]): Rect[] {
  const n = values.length;
  const out: Rect[] = new Array(n);
  if (n === 0) return out;

  const total = values.reduce((sum, value) => sum + value, 0);
  if (!(total > 0)) return values.map(() => ({ x: 0, y: 0, w: 0, h: 0 }));

  const items = values
    .map((value, i) => ({ area: value / total, i }))
    .sort((a, b) => b.area - a.area || a.i - b.i);

  // The part of the 1×1 box still to be filled.
  let x = 0;
  let y = 0;
  let w = 1;
  let h = 1;

  // Places one row — items `from..to` — as a strip along the current shorter side, and shrinks
  // the remaining box by that strip.
  const layoutRow = (from: number, to: number) => {
    let rowArea = 0;
    for (let i = from; i < to; i++) rowArea += items[i].area;
    if (w >= h) {
      // The strip grows in x; each item is a vertical slice of the full height.
      const thickness = h <= 0 ? 0 : rowArea / h;
      let cursor = y;
      for (let i = from; i < to; i++) {
        const length = rowArea <= 0 ? 0 : (items[i].area / rowArea) * h;
        out[items[i].i] = { x, y: cursor, w: thickness, h: length };
        cursor += length;
      }
      x += thickness;
      w -= thickness;
    } else {
      // The strip grows in y; each item is a horizontal slice of the full width.
      const thickness = w <= 0 ? 0 : rowArea / w;
      let cursor = x;
      for (let i = from; i < to; i++) {
        const length = rowArea <= 0 ? 0 : (items[i].area / rowArea) * w;
        out[items[i].i] = { x: cursor, y, w: length, h: thickness };
        cursor += length;
      }
      y += thickness;
      h -= thickness;
    }
  };

  // The worst aspect ratio of the row `from..to` laid along the current shorter side.
  const worst = (from: number, to: number): number => {
    let sum = 0;
    let rMax = 0;
    let rMin = Infinity;
    for (let i = from; i < to; i++) {
      const a = items[i].area;
      sum += a;
      if (a > rMax) rMax = a;
      if (a < rMin) rMin = a;
    }
    const short = Math.min(w, h);
    if (sum <= 0 || short <= 0 || rMin <= 0) return Infinity;
    const side = short * short;
    return Math.max((side * rMax) / (sum * sum), (sum * sum) / (side * rMin));
  };

  let rowStart = 0;
  while (rowStart < n) {
    let end = rowStart + 1;
    while (end < n && worst(rowStart, end + 1) <= worst(rowStart, end)) end++;
    layoutRow(rowStart, end);
    rowStart = end;
  }

  return out;
}
