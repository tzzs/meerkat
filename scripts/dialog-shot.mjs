// Opens the quit dialog, screenshots it, then cancels — nothing is ever confirmed here.
import { writeFileSync } from "node:fs";
const PORT = process.env.PULSE_CDP ?? "9334";
const targets = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json());
const page = targets.find((t) => t.type === "page" && !t.url.includes("panel"));
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => socket.addEventListener("open", r));
let id = 0;
const pending = new Map();
socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
const send = (method, params) => { const n = ++id; socket.send(JSON.stringify({ id: n, method, params })); return new Promise((r) => pending.set(n, r)); };
const ev = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;
await send("Page.enable");
console.log(await ev(`(() => { const q = document.querySelector(".row .row-quit"); q.click(); return q.closest(".row").querySelector(".row-name").textContent; })()`));
await new Promise((r) => setTimeout(r, 500));
const shot = await send("Page.captureScreenshot", { format: "png" });
writeFileSync("shots/dialog-radix.png", Buffer.from(shot.result.data, "base64"));
console.log(await ev(`(() => { const b = [...document.querySelectorAll('[role="dialog"] button')][0]; const t = b.textContent; b.click(); return "cancelled: " + t; })()`));
socket.close();
