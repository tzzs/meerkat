// Checks the mixer fader in the real window: what the Radix slider is actually wearing, and
// whether the hand still gets an immediate answer from it.
//
//   PULSE_CDP=9334 node scripts/slider-probe.mjs styles
//   PULSE_CDP=9334 node scripts/slider-probe.mjs keys   "立体声混音"
//   PULSE_CDP=9334 node scripts/slider-probe.mjs drag   "立体声混音"
//   PULSE_CDP=9334 node scripts/slider-probe.mjs drag   "立体声混音" 55
//
// `keys` and `drag` change a volume, so they take the row to change by name and put it back
// afterwards; point them at a device nothing is playing through, never at an app. A fourth
// argument overrides the value to restore to, for tidying up after an earlier run.
import { mkdirSync, writeFileSync } from "node:fs";

const PORT = process.env.PULSE_CDP ?? "9334";
const phase = process.argv[2] ?? "styles";
const rowName = process.argv[3] ?? "";

const targets = await fetch(`http://127.0.0.1:${PORT}/json/list`)
  .then((r) => r.json())
  .catch(() => null);
if (!targets) {
  console.log(`no DevTools endpoint on ${PORT}`);
  process.exit(1);
}
const page = targets.find((t) => t.type === "page" && !t.url.includes("panel"));
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 1;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
});
const send = (method, params = {}) => {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
};
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.result?.exceptionDetails) return `EXCEPTION ${JSON.stringify(r.result.exceptionDetails)}`;
  return r.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send("Page.enable");
if ((await evaluate("document.body.innerText.trim().length")) < 4) {
  await send("Page.reload", { ignoreCache: true });
  await wait(2500);
}

// Land on the mixer without reloading, so whatever the user had open stays open.
await evaluate(
  `(() => { const b = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim() === "声音");
     if (b) { b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true })); b.click(); }
     return Boolean(b); })()`,
);
await wait(1200);

if (phase === "styles") {
  const read = await evaluate(`(() => {
    const pick = (el, keys) => { const c = getComputedStyle(el); const o = {};
      for (const k of keys) o[k] = c[k]; return o; };
    const thumb = document.querySelector('[role="slider"]');
    if (!thumb) return "no slider on screen";
    const root = thumb.parentElement.parentElement;   // wrapper span -> Root
    const track = root.querySelector('[data-orientation="horizontal"]');
    const range = track?.firstElementChild;
    const overlay = range?.nextElementSibling;
    const rows = [...document.querySelectorAll(".mix-row")].map((r) => ({
      name: r.querySelector(".mix-label")?.textContent?.trim() ?? "",
      value: r.querySelector(".mix-value.num")?.textContent?.trim() ?? "",
      slider: Boolean(r.querySelector('[role="slider"]')),
    }));
    return {
      aria: { role: thumb.getAttribute("role"), label: thumb.getAttribute("aria-label"),
              now: thumb.getAttribute("aria-valuenow"), min: thumb.getAttribute("aria-valuemin"),
              max: thumb.getAttribute("aria-valuemax"), tabIndex: thumb.tabIndex },
      root: pick(root, ["position", "display", "height", "alignItems", "touchAction", "userSelect"]),
      track: track && pick(track, ["position", "height", "borderRadius", "backgroundColor", "overflow"]),
      range: range && pick(range, ["position", "height", "backgroundColor", "transitionProperty",
                                   "transitionDuration", "transitionTimingFunction", "left", "right"]),
      overlay: overlay && pick(overlay, ["position", "backgroundColor", "transform", "transition"]),
      thumb: pick(thumb, ["position", "width", "height", "borderRadius", "backgroundColor",
                          "borderTopWidth", "borderTopColor", "boxShadow", "transform", "transition"]),
      geometry: {
        rootRect: root.getBoundingClientRect().toJSON(),
        trackRect: track?.getBoundingClientRect().toJSON(),
        thumbRect: thumb.getBoundingClientRect().toJSON(),
      },
      rows: rows.slice(0, 12),
      rowHeights: [...new Set([...document.querySelectorAll(".mix-row")].map((r) => Math.round(r.getBoundingClientRect().height)))],
    };
  })()`);
  console.log(JSON.stringify(read, null, 1));

  const shot = await send("Page.captureScreenshot", { format: "png" });
  mkdirSync("shots", { recursive: true });
  writeFileSync("shots/slider-sound.png", Buffer.from(shot.result.data, "base64"));
  console.log("wrote shots/slider-sound.png");
  process.exit(0);
}

if (!rowName) {
  console.log("pass the row to exercise, e.g. 立体声混音");
  process.exit(1);
}

const find = `(() => {
  const row = [...document.querySelectorAll(".mix-row")]
    .find((r) => (r.querySelector(".mix-label")?.textContent ?? "").includes(${JSON.stringify(rowName)}));
  return row ? row.querySelector('[role="slider"]') : null;
})()`;

const state = async () =>
  evaluate(`(() => { const t = ${find}; if (!t) return null;
    const row = t.closest(".mix-row");
    const range = t.parentElement.parentElement.querySelector('[data-orientation="horizontal"]')?.firstElementChild;
    return { aria: t.getAttribute("aria-valuenow"),
             shown: row.querySelector(".mix-value.num")?.textContent?.trim(),
             thumbLeft: getComputedStyle(t.parentElement).left,
             rangeRight: range ? getComputedStyle(range).right : null }; })()`);

const before = await state();
console.log("before:", JSON.stringify(before));
if (!before) process.exit(1);

if (phase === "keys") {
  await evaluate(`(() => { const t = ${find}; t.focus(); return document.activeElement === t; })()`);
  for (const key of ["ArrowRight", "ArrowUp", "PageUp", "Home", "End", "ArrowLeft"]) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key });
    await wait(120);
    const s = await state();
    console.log(`${key.padEnd(10)} -> aria ${String(s.aria).padStart(3)} shown ${s.shown} thumbLeft ${s.thumbLeft}`);
  }
}

if (phase === "drag") {
  const rect = await evaluate(`(() => { const t = ${find};
    const r = t.parentElement.parentElement.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height }; })()`);
  const y = rect.y + rect.h / 2;
  const at = (frac) => rect.x + rect.w * frac;
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: at(0.3), y, button: "left", clickCount: 1 });
  await wait(100);
  console.log("press at 30%:", JSON.stringify(await state()));
  for (const frac of [0.42, 0.55, 0.62, 0.48]) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at(frac), y, button: "left" });
    await wait(70);
    const s = await state();
    console.log(`move to ${(frac * 100).toFixed(0)}% (no wait for the poll) ->`, JSON.stringify(s));
  }
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: at(0.48), y, button: "left", clickCount: 1 });
  await wait(600);
  console.log("after release:", JSON.stringify(await state()));
  console.log("poll still suppressed during drag?", await evaluate("document.body.innerText.length > 0"));
}

// Put it back exactly where it was, through the same command the UI uses. One click at the
// target fraction writes the value in a single step; a keyboard walk dropped a key under load.
// A fourth argument overrides the target, for putting a row back after an earlier run left it
// somewhere else.
await wait(900);
const target = process.argv[4] ? Number(process.argv[4]) : Number(String(before.aria));
const restored = await evaluate(`(() => {
  const row = ${find}.closest(".mix-row");
  return { name: row.querySelector(".mix-label")?.textContent?.trim() };
})()`);
console.log("restoring", JSON.stringify(restored), "to", target);
const rect = await evaluate(`(() => { const t = ${find};
  const r = t.parentElement.parentElement.getBoundingClientRect();
  return { x: r.x, y: r.y, w: r.width, h: r.height }; })()`);
const x = rect.x + rect.w * (target / 100);
const y = rect.y + rect.h / 2;
await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
await wait(900);
let after = await state();
const nudge = target - Number(after.aria);
if (nudge !== 0) {
  await evaluate(`(() => { ${find}.focus(); return true; })()`);
  for (let i = 0; i < Math.abs(nudge); i++) {
    const key = nudge > 0 ? "ArrowRight" : "ArrowLeft";
    await send("Input.dispatchKeyEvent", { type: "keyDown", key });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key });
    await wait(150);
  }
  after = await state();
}
console.log("after restore:", JSON.stringify(after));
process.exit(0);
