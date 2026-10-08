// Measures whether a rate figure holds still between samples. With easing on, one 1-second
// sample produced several different strings while the number slid towards it; the point of this
// is to show that it now changes exactly once per sample and never mid-interval.
const targets = await fetch("http://127.0.0.1:9334/json/list").then((r) => r.json());
const page = targets.find((t) => t.type === "page" && !t.url.includes("panel"));
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => socket.addEventListener("open", r));
let id = 0;
const pending = new Map();
socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  }
});
const send = (method, params) => {
  const n = ++id;
  socket.send(JSON.stringify({ id: n, method, params }));
  return new Promise((r) => pending.set(n, r));
};
const ev = async (expression) =>
  (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;

const body = `(async () => {
  const read = () => [...document.querySelectorAll(".rate-pair .huge.num")].map((e) => e.textContent.trim());
  const samples = [];
  for (let i = 0; i < 80; i++) {
    samples.push(read().join(" | "));
    await new Promise((r) => setTimeout(r, 150));
  }
  let changes = 0;
  for (let i = 1; i < samples.length; i++) if (samples[i] !== samples[i - 1]) changes++;
  const units = new Set();
  for (const s of samples) for (const part of s.split(" ")) if (/B\\/s|bit\\/s|KB|MB|GB/i.test(part) && !/^[0-9.,]+$/.test(part)) units.add(part);
  return JSON.stringify({
    windowSeconds: Math.round((samples.length * 150) / 1000),
    distinctStrings: new Set(samples).size,
    changesBetweenReads: changes,
    unitsSeen: [...units],
    first: samples[0],
    last: samples[samples.length - 1],
  });
})()`;

console.log(await ev(body));
socket.close();
