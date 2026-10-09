// Screenshots a running Pulse webview through the DevTools protocol, so design work is
// checked against the real window rather than against a description of it.
//
//   node scripts/ui-shot.mjs http://localhost:1420/?tab=cpu shots/browser.png
//   PULSE_CDP=9334 node scripts/ui-shot.mjs same shots/app-cpu.png 2500 0 处理器
//
// Port 9333 is the plain-browser mirror started by scripts/open-probe.ps1; 9334 is the app's
// own WebView2, which needs WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9334
// in the environment that launches `pnpm tauri dev`. Dev only: nothing ships with it.
import { mkdirSync, writeFileSync } from "node:fs";

const PORT = process.env.PULSE_CDP ?? "9333";
const url = process.argv[2] ?? "http://localhost:1420/";
const out = process.argv[3] ?? "shots/shot.png";
const settleMs = Number(process.argv[4] ?? 2500);
const scroll = process.argv[5] ?? "";
const tabLabel = process.argv[6] ?? "";

// The endpoint is briefly unavailable while the app restarts after a Rust rebuild, so ask a
// few times before giving up.
let targets = null;
for (let attempt = 0; attempt < 12 && !targets; attempt++) {
  targets = await fetch(`http://127.0.0.1:${PORT}/json/list`)
    .then((r) => r.json())
    .catch(() => null);
  if (!targets) await new Promise((resolve) => setTimeout(resolve, 1500));
}
if (!targets) {
  console.log(`no DevTools endpoint on ${PORT}`);
  process.exit(1);
}
const page = targets.find((t) => t.type === "page" && (out.includes("panel") ? t.url.includes("panel") : !t.url.includes("panel")));
if (!page) {
  console.log("no page target;", targets.map((t) => t.url).join(", "));
  process.exit(1);
}

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
  const r = await send("Runtime.evaluate", { expression, returnByValue: true });
  if (r.result?.exceptionDetails) return `EXCEPTION ${r.result.exceptionDetails.text}`;
  return r.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send("Page.enable");
// A failed HMR update can leave the webview with an unmounted tree and no way back; when the
// body is empty, reload once rather than screenshotting a blank window and calling it a bug.
if ((await evaluate("document.body.innerText.trim().length")) < 4) {
  await send("Page.reload", { ignoreCache: true });
  await wait(settleMs);
}
// "same" keeps the window where the user left it; clicking a tab exercises the real control
// instead of reloading the page and losing whatever state the tab holds.
if (url !== "same" && url !== page.url) {
  await send("Page.navigate", { url });
  await wait(settleMs);
}
if (tabLabel) {
  const clicked = await evaluate(
    `(() => { const b = [...document.querySelectorAll(".tab")]
        .find((n) => n.textContent.trim() === ${JSON.stringify(tabLabel)});
      if (b) b.click(); return Boolean(b); })()`,
  );
  if (!clicked) console.log(`no tab labelled ${tabLabel}`);
  await wait(settleMs);
}
if (scroll) {
  await evaluate(
    `(() => { const c = document.querySelector(".content");
      c.scrollTop = ${scroll === "bottom" ? "c.scrollHeight" : Number(scroll)}; return c.scrollTop; })()`,
  );
  await wait(700);
}

const shot = await send("Page.captureScreenshot", { format: "png" });
const base64 = shot.result?.data;
if (!base64) {
  console.log("capture failed:", JSON.stringify(shot.error ?? shot).slice(0, 200));
  process.exit(1);
}
mkdirSync(out.slice(0, out.lastIndexOf("/")), { recursive: true });
const bytes = Buffer.from(base64, "base64");
writeFileSync(out, bytes);
console.log(
  out,
  bytes.length,
  "bytes |",
  await evaluate(
    `JSON.stringify({w: innerWidth, h: innerHeight, rows: document.querySelectorAll(".row").length,
      icons: document.querySelectorAll("img.app-icon").length,
      monograms: document.querySelectorAll(".mono-avatar").length})`,
  ),
);
socket.close();
