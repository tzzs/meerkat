// Drives the Performance tab through the whole chain on the running app: the sessions a real
// data dir brings, opening the longest one, recording a short session and stopping it, and then
// checks what the console is supposed to keep — a report small enough to send, a store that does
// not grow with the length of a session, a chart a person can read, and no journal left behind.
//
// The DevTools endpoint has to be enabled on the app being tested; MEERKAT_DATA_DIR, when set,
// is the directory that app was started with, so the files can be measured from here.
//
//   MEERKAT_CDP=9334 MEERKAT_DATA_DIR=%TEMP%\meerkat node scripts/perf-probe.mjs
import { mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const PORT = process.env.MEERKAT_CDP ?? "9334";
const DATA_DIR = process.env.MEERKAT_DATA_DIR ?? "";
/// A saved session's report is a few kilobytes; this is generous enough to catch a regression
/// that starts shipping the readings again (a twenty-hour session was 17 MB).
const MAX_REPORT_BYTES = 256 * 1024;
const MAX_STORE_BYTES = 512 * 1024;
/// Matches `SERIES_POINTS` in src-tauri/src/perf.rs: the chart is 240 units wide.
const SERIES_POINTS = 240;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

let targets = null;
for (let attempt = 0; attempt < 40 && !targets; attempt++) {
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
if (!page) {
  console.log("no main page target", targets.map((target) => target.url).join(", "));
  process.exit(1);
}

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
let nextId = 1;
const pending = new Map();
const problems = [];
socket.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
  if (msg.method === "Runtime.exceptionThrown") {
    problems.push(`throw: ${String(msg.params.exceptionDetails.exception?.description ?? "").slice(0, 160)}`);
  }
});
const send = (method, params = {}) => {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
};
const run = async (fn, ...args) => {
  const expression = `(${fn.toString()})(${args.map((argument) => JSON.stringify(argument)).join(",")})`;
  const response = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (response.result?.exceptionDetails) {
    throw new Error(String(response.result.exceptionDetails.exception?.description ?? "evaluate failed"));
  }
  return response.result?.result?.value;
};

await send("Page.enable");
await send("Runtime.enable");
await wait(2500);

const labels = await run(() => [...document.querySelectorAll('[role="tab"]')].map((node) => node.textContent.trim()));
if (!labels.length) {
  console.log("the window is not showing the app", JSON.stringify({ labels }));
  process.exit(1);
}
const tab = labels.find((label) => /性能|Performance/i.test(label));
if (!tab) {
  console.log("no performance tab", labels.join(" | "));
  process.exit(1);
}

// ---- the tab opens with the saved sessions ------------------------------------------------
const opened = await run(async (tabLabel) => {
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const button = [...document.querySelectorAll('[role="tab"]')].find((node) => node.textContent.trim() === tabLabel);
  const started = performance.now();
  if (button && button.getAttribute("data-state") !== "active") {
    button.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    button.click();
  }
  for (let attempt = 0; attempt < 400; attempt++) {
    if (document.querySelectorAll(".perf-table tbody tr").length > 0 || document.querySelector(".perf-history .empty")) break;
    await sleep(25);
  }
  return {
    ms: Math.round(performance.now() - started),
    rows: [...document.querySelectorAll(".perf-table tbody tr")].map((row) =>
      (row.textContent ?? "").replace(/\s+/g, " ").trim(),
    ),
  };
}, tab);
if (!opened.rows.length) problems.push("the history table listed no saved session");

// ---- what the backend sends ---------------------------------------------------------------
const payloads = await run(async () => {
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  if (!invoke) return { error: "no invoke" };
  const summaries = await invoke("perf_sessions");
  const measured = [];
  for (const summary of (summaries ?? []).slice(0, 2)) {
    const started = performance.now();
    const session = await invoke("perf_report", { id: summary.id });
    measured.push({
      id: summary.id,
      samples: summary.sampleCount,
      ms: Math.round(performance.now() - started),
      bytes: JSON.stringify(session ?? null).length,
      series: (session?.series ?? []).map((entry) => [entry.key, entry.values.length]),
    });
  }
  return { count: (summaries ?? []).length, measured };
});
if (payloads.error) {
  problems.push(`could not read the sessions over IPC: ${payloads.error}`);
} else {
  for (const report of payloads.measured) {
    if (report.bytes > MAX_REPORT_BYTES) {
      problems.push(`a ${report.samples}-sample report was ${report.bytes} bytes (> ${MAX_REPORT_BYTES})`);
    }
    if (report.ms > 200) problems.push(`a saved report took ${report.ms} ms to come back`);
    for (const [key, points] of report.series) {
      if (points > SERIES_POINTS) problems.push(`the ${key} series had ${points} points (> ${SERIES_POINTS})`);
    }
  }
}

// ---- opening the longest saved result ------------------------------------------------------
const sheet = await run(async () => {
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const close = [...document.querySelectorAll("button")].find((node) => /关闭|Close/.test(node.getAttribute("aria-label") ?? ""));
  if (document.querySelector('[role="dialog"]')) {
    close?.click();
    await sleep(300);
  }
  const rows = [...document.querySelectorAll(".perf-table tbody tr")];
  const row = rows.find((node) => /小时|hours/.test(node.textContent ?? "")) ?? rows[0];
  if (!row) return { skipped: "no row to open" };
  const button = [...row.querySelectorAll("button")].find((node) => /查看结果|View/.test(node.textContent ?? ""));
  const started = performance.now();
  button?.click();
  let appeared = null;
  for (let attempt = 0; attempt < 400; attempt++) {
    const dialog = document.querySelector('[role="dialog"]');
    if (dialog && /结论|Verdict|平均|Average/.test(dialog.textContent ?? "")) {
      appeared = Math.round(performance.now() - started);
      break;
    }
    await sleep(25);
  }
  const dialog = document.querySelector('[role="dialog"]');
  const paths = [...(dialog?.querySelectorAll("svg.chart path[stroke]") ?? [])].map((path) => path.getAttribute("d") ?? "");
  return {
    ms: appeared,
    charts: dialog?.querySelectorAll("svg.chart").length ?? 0,
    curved: paths.filter((path) => path.includes(" C")).length,
    straight: paths.filter((path) => path.includes(" C") === false).length,
    segments: (paths[0]?.match(/ C/g) ?? []).length,
    text: (dialog?.textContent ?? "").replace(/\s+/g, " ").slice(0, 200),
  };
});
if (sheet.skipped) {
  problems.push(`could not open a saved result: ${sheet.skipped}`);
} else {
  if (sheet.ms === null) problems.push("the result sheet never showed its verdict");
  else if (sheet.ms > 2000) problems.push(`the result sheet took ${sheet.ms} ms to show`);
  if (sheet.charts < 8) problems.push(`the sheet drew ${sheet.charts} charts`);
  if (sheet.curved === 0) problems.push("the sheet's lines are still straight segments");
  if (sheet.segments > SERIES_POINTS) problems.push(`the first chart had ${sheet.segments} segments (> ${SERIES_POINTS})`);
}

// ---- one more session, recorded and stopped ------------------------------------------------
const before = await run(() => document.querySelectorAll(".perf-table tbody tr").length);
const recorded = await run(async () => {
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const close = [...document.querySelectorAll("button")].find((node) => /关闭|Close/.test(node.getAttribute("aria-label") ?? ""));
  if (document.querySelector('[role="dialog"]')) {
    close?.click();
    await sleep(300);
  }
  const find = (pattern) => [...document.querySelectorAll("button")].find((node) => pattern.test((node.textContent ?? "").trim()) && !node.disabled);
  const start = find(/^开始$|^Start$/);
  if (!start) return { started: false, text: document.querySelector(".perf-session")?.textContent?.replace(/\s+/g, " ") ?? "" };
  start.click();
  for (let attempt = 0; attempt < 80; attempt++) {
    if (/正在|Recording|Live/i.test(document.querySelector(".perf-session")?.textContent ?? "")) break;
    await sleep(100);
  }
  await sleep(4000);
  const live = document.querySelector(".perf-session")?.textContent?.replace(/\s+/g, " ").slice(0, 120) ?? "";
  const stop = find(/^停止$|^Stop$/);
  if (!stop) return { started: true, stopped: false, live };
  stop.click();
  for (let attempt = 0; attempt < 120; attempt++) {
    if (document.querySelectorAll(".perf-table tbody tr").length > 0) break;
    await sleep(100);
  }
  await sleep(800);
  return {
    started: true,
    stopped: true,
    live,
    rows: document.querySelectorAll(".perf-table tbody tr").length,
  };
});
await wait(500);
const rowsNow = await run(() => document.querySelectorAll(".perf-table tbody tr").length);
if (!recorded.started) problems.push(`could not start a session: ${recorded.text ?? ""}`);
else if (!recorded.stopped) problems.push("a started session could not be stopped");
else if (rowsNow <= before) problems.push(`stopping added no row (${before} before, ${rowsNow} after)`);

// ---- what the data dir looks like afterwards -----------------------------------------------
let store = null;
let journals = null;
if (DATA_DIR) {
  try {
    store = statSync(join(DATA_DIR, "perf-sessions.json")).size;
  } catch {
    problems.push(`no perf-sessions.json in ${DATA_DIR}`);
  }
  try {
    journals = readdirSync(join(DATA_DIR, "perf-samples")).filter((name) => name.endsWith(".jsonl"));
  } catch {
    journals = [];
  }
  if (store !== null && store > MAX_STORE_BYTES) {
    problems.push(`perf-sessions.json is ${store} bytes (> ${MAX_STORE_BYTES})`);
  }
  if (journals.length > 0) problems.push(`a finished session left ${journals.length} journal(s) behind`);
}

const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync("shots/perf-console.png", Buffer.from(shot.result.data, "base64"));

console.log(
  JSON.stringify(
    {
      tab,
      savedSessions: opened.rows.length,
      tabOpenMs: opened.ms,
      payloads,
      sheet,
      recorded,
      rowsNow,
      store,
      journals,
      problems,
    },
    null,
    2,
  ),
);
socket.close();
process.exit(problems.length > 0 ? 1 : 0);
