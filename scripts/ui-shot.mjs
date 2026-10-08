// Screenshots the running UI through the probe window's DevTools session, so design work can
// be checked against the real snapshot instead of against a description of it.
//   node scripts/ui-shot.mjs http://localhost:1420/?tab=cpu shots/cpu.png
import { mkdirSync } from "node:fs";

const url = process.argv[2] ?? "http://localhost:1420/";
const out = process.argv[3] ?? "shots/shot.png";
const settleMs = Number(process.argv[4] ?? 2500);

const list = await fetch("http://127.0.0.1:9333/json/list").then((r) => r.json());
const page = list.find((t) => t.type === "page");
if (!page) {
  console.log("no probe page; run scripts/open-probe.ps1 first");
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
  return r.result?.result?.value;
};

await send("Page.enable");
await send("Page.navigate", { url });
await new Promise((resolve) => setTimeout(resolve, settleMs));

// "bottom" is the interesting part of a long list: it is where a virtualised table either
// shows the last row or shows a blank gap.
const scroll = process.argv[5];
if (scroll) {
  await evaluate(
    `(() => { const c = document.querySelector(".content");
      c.scrollTop = ${scroll === "bottom" ? "c.scrollHeight" : Number(scroll)}; return c.scrollTop; })()`,
  );
  await new Promise((resolve) => setTimeout(resolve, 600));
}

const shot = await send("Page.captureScreenshot", { format: "png" });
const base64 = shot.result?.data;
if (!base64) {
  console.log("capture failed:", JSON.stringify(shot.error ?? shot).slice(0, 200));
  process.exit(1);
}
mkdirSync(out.slice(0, out.lastIndexOf("/")), { recursive: true });
const bytes = Buffer.from(base64, "base64");
await import("node:fs").then(({ writeFileSync }) => writeFileSync(out, bytes));
console.log(
  out,
  bytes.length,
  "bytes |",
  await evaluate("JSON.stringify({w: innerWidth, h: innerHeight, rows: document.querySelectorAll('.row').length})"),
);
socket.close();
