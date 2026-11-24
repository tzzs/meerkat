// Captures the browser mirror at one tab and reports whether hairline chrome is drawing.
//
//   node scripts/mirror-shot.mjs <out.png> [tabLabel] [port]
const [out, tabLabel = "性能", port = "9455"] = process.argv.slice(2);
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const target = await fetch(`http://127.0.0.1:${port}/json/new?http://localhost:1420/`, { method: "PUT" })
  .then((r) => r.json())
  .catch(() => null);
if (!target) {
  console.log("could not open a tab");
  process.exit(1);
}

const socket = new WebSocket(target.webSocketDebuggerUrl);
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
const run = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (r.result?.exceptionDetails) return { error: String(r.result.exceptionDetails.exception?.description ?? "failed") };
  return r.result?.result?.value;
};

await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
await wait(2500);

const clicked = await run(`(() => {
  const b = [...document.querySelectorAll('[role=tab]')].find((n) => n.textContent.trim() === ${JSON.stringify(tabLabel)});
  if (!b) return false;
  b.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
  b.click();
  return true;
})()`);
await wait(1200);

// The metric the change is about: does a rule that asks for a line colour actually draw one?
const probe = await run(`(() => {
  const css = (el, prop) => el ? getComputedStyle(el)[prop] : null;
  const find = document.querySelector('.titlebar .find');
  const thumb = [...document.styleSheets].flatMap((s) => { try { return [...s.cssRules]; } catch { return []; } })
    .filter((r) => r.selectorText && r.selectorText.includes('scrollbar-thumb'))
    .map((r) => r.style.background || r.style.backgroundColor);
  return {
    titlebarFindBorder: css(find, 'borderTopWidth') + ' ' + css(find, 'borderTopStyle') + ' ' + css(find, 'borderTopColor'),
    titlebarFindBorderWidth: css(find, 'borderTopWidth'),
    scrollbarThumbRule: thumb,
    cards: document.querySelectorAll('section').length,
    rows: document.querySelectorAll('.row').length,
    appsInPicker: document.querySelectorAll('.perf-app').length,
    bodyText: document.body.innerText.trim().slice(0, 80),
  };
})()`);

const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, Buffer.from(shot.result.data, "base64"));
console.log(JSON.stringify({ out, clicked, probe }, null, 2));
socket.close();
