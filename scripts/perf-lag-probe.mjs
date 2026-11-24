// Measures how heavy Meerkat's window is while the Performance tab is open: long tasks,
// frame gaps while idle and while scrolling the app picker, and the main-thread time the
// renderer spends per second. Meant for a debug build started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=NNNN.
//
//   MEERKAT_CDP=9339 node scripts/perf-lag-probe.mjs [tab]
import { writeFileSync } from "node:fs";

const PORT = process.env.MEERKAT_CDP ?? "9339";
const TAB = process.argv[2] ?? "perf";
const OUT = process.argv[3];
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let targets = null;
for (let attempt = 0; attempt < 40 && !targets; attempt++) {
  targets = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json()).catch(() => null);
  if (!targets) await wait(500);
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
const evaluate = async (expression) => {
  const response = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (response.result?.exceptionDetails) {
    throw new Error(String(response.result.exceptionDetails.exception?.description ?? "eval failed"));
  }
  return response.result?.result?.value;
};

await send("Page.enable");
await send("Runtime.enable");
await send("Performance.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1120, height: 740, deviceScaleFactor: 1.75, mobile: false });

const target = new URL(page.url);
target.searchParams.set("tab", TAB);
await send("Page.navigate", { url: target.toString() });
await wait(4000);

const readMetrics = async () => {
  const response = await send("Performance.getMetrics");
  const wanted = ["ScriptDuration", "LayoutDuration", "RecalcStyleDuration", "TaskDuration", "Nodes", "JSHeapUsedSize"];
  const map = {};
  for (const metric of response.result.metrics) {
    if (wanted.includes(metric.name)) map[metric.name] = metric.value;
  }
  return map;
};

// Wait for the tab's content, not just the shell.
for (let attempt = 0; attempt < 40; attempt++) {
  const ready = await evaluate(`Boolean(document.querySelector(".perf-apps") || document.querySelector(".metric-card"))`);
  if (ready) break;
  await wait(250);
}

// The picker's rows only exist in the "monitor one app" mode; the console opens on "follow
// the foreground window", which draws two lines of text and nothing else.
const picker = await evaluate(`(async () => {
  if (document.querySelector(".perf-apps")) return "already";
  const chip = [...document.querySelectorAll(".perf-mode [role=\\"radio\\"], .perf-mode button")].find((b) =>
    /监控指定应用|Monitor an app/.test(b.textContent ?? ""),
  );
  if (!chip) return "no chip";
  chip.click();
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  for (let i = 0; i < 40; i++) {
    if (document.querySelector(".perf-app")) return "opened";
    await sleep(100);
  }
  return "no rows";
})()`);
await wait(1500);

const before = await readMetrics();
const idle = await evaluate(`(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const gaps = [];
  let last = performance.now();
  let running = true;
  const frame = (now) => {
    gaps.push(now - last);
    last = now;
    if (running) requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
  const long = [];
  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) long.push(entry.duration);
  });
  try { observer.observe({ entryTypes: ["longtask"] }); } catch {}
  await sleep(4000);
  running = false;
  observer.disconnect();
  return {
    gapCount: gaps.length,
    maxGap: Math.max(...gaps).toFixed(1),
    p95Gap: gaps.slice().sort((a, b) => a - b)[Math.floor(gaps.length * 0.95)]?.toFixed(1),
    longTasks: long.length,
    longTotal: long.reduce((a, b) => a + b, 0).toFixed(0),
  };
})()`);
const afterIdle = await readMetrics();

// Scroll the picker for a few seconds, which is what a person does with 95 rows on screen.
const scrolling = await evaluate(`(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const list = document.querySelector(".perf-apps");
  if (!list) return { skipped: "no app list" };
  const gaps = [];
  let last = performance.now();
  let running = true;
  const frame = (now) => {
    gaps.push(now - last);
    last = now;
    if (running) requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
  const started = performance.now();
  let direction = 1;
  while (performance.now() - started < 4000) {
    list.scrollTop += direction * 40;
    if (list.scrollTop <= 0) direction = 1;
    if (list.scrollTop >= list.scrollHeight - list.clientHeight) direction = -1;
    await sleep(16);
  }
  running = false;
  const sorted = gaps.slice().sort((a, b) => a - b);
  return {
    gapCount: gaps.length,
    maxGap: Math.max(...gaps).toFixed(1),
    p95Gap: sorted[Math.floor(sorted.length * 0.95)]?.toFixed(1),
    rows: list.querySelectorAll(".perf-app").length,
    scrollHeight: list.scrollHeight,
  };
})()`);
const afterScroll = await readMetrics();

const dom = await evaluate(`({
  nodes: document.getElementsByTagName("*").length,
  rows: document.querySelectorAll(".perf-app").length,
  icons: document.querySelectorAll(".perf-app img.app-icon").length,
  tab: document.querySelector('[role="tab"][data-state="active"]')?.textContent?.trim() ?? "",
})`);

if (OUT) {
  const shot = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(OUT, Buffer.from(shot.result.data, "base64"));
}
const delta = (a, b) => (b - a).toFixed(3);
console.log(
  JSON.stringify(
    {
      tab: TAB,
      picker,
      dom,
      idlePerSecond: {
        scriptMs: (Number(delta(before.ScriptDuration, afterIdle.ScriptDuration)) / 4 * 1000).toFixed(1),
        layoutMs: (Number(delta(before.LayoutDuration, afterIdle.LayoutDuration)) / 4 * 1000).toFixed(1),
        styleMs: (Number(delta(before.RecalcStyleDuration, afterIdle.RecalcStyleDuration)) / 4 * 1000).toFixed(1),
        taskMs: (Number(delta(before.TaskDuration, afterIdle.TaskDuration)) / 4 * 1000).toFixed(1),
      },
      idle,
      scrolling,
      scrollPerSecond: {
        scriptMs: (Number(delta(afterIdle.ScriptDuration, afterScroll.ScriptDuration)) / 4 * 1000).toFixed(1),
        layoutMs: (Number(delta(afterIdle.LayoutDuration, afterScroll.LayoutDuration)) / 4 * 1000).toFixed(1),
        styleMs: (Number(delta(afterIdle.RecalcStyleDuration, afterScroll.RecalcStyleDuration)) / 4 * 1000).toFixed(1),
      },
    },
    null,
    2,
  ),
);
socket.close();
