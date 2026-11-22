// Reads the live performance session state from the running webview over the DevTools protocol.
//
//   MEERKAT_CDP=9334 node scripts/perf-status.mjs
const PORT = process.env.MEERKAT_CDP ?? "9334";
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let targets = null;
for (let attempt = 0; attempt < 20 && !targets; attempt++) {
  targets = await fetch(`http://127.0.0.1:${PORT}/json/list`)
    .then((r) => r.json())
    .catch(() => null);
  if (!targets) await wait(1000);
}
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

const response = await send("Runtime.evaluate", {
  expression: `window.__TAURI_INTERNALS__.invoke("perf_status")`,
  awaitPromise: true,
  returnByValue: true,
});
if (response.result?.exceptionDetails) {
  console.log(JSON.stringify({ error: String(response.result.exceptionDetails.exception?.description ?? "invoke failed") }));
  process.exit(1);
}
console.log(JSON.stringify(response.result?.result?.value));
socket.close();
