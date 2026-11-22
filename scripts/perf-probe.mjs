// Drives the Game performance tab through one full session on the running app and reads back
// what a person would see. The DevTools endpoint has to be enabled on the app being tested.
//
//   MEERKAT_CDP=9334 node scripts/perf-probe.mjs
import { mkdirSync, writeFileSync } from "node:fs";

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
if (!page) {
  console.log("no main page target", targets.map((t) => t.url).join(", "));
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
  const expression = `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(",")})`;
  const response = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (response.result?.exceptionDetails) {
    throw new Error(String(response.result.exceptionDetails.exception?.description ?? "evaluate failed"));
  }
  return response.result?.result?.value;
};

await send("Page.enable");
await send("Runtime.enable");
await wait(4000);

const before = await run(() => ({
  title: document.title,
  lang: navigator.language,
  labels: [...document.querySelectorAll('[role="tab"]')].map((n) => n.textContent.trim()),
}));
if (!before.labels.length) {
  console.log("the window is not showing the app", JSON.stringify(before));
  process.exit(1);
}
const tab = ["Game performance", "游戏性能", "性能"].find((label) => before.labels.includes(label));
if (!tab) {
  console.log("no performance tab", before.labels.join(" | "));
  process.exit(1);
}

const clicked = await run((label) => {
  const button = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim() === label);
  if (!button) return false;
  button.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  button.click();
  return true;
}, tab);
if (!clicked) throw new Error("could not click the performance tab");
await wait(1200);

const press = async (labels) => {
  return run((candidates) => {
    const button = [...document.querySelectorAll('button, [role="button"]')].find((node) =>
      candidates.includes((node.textContent ?? "").trim()),
    );
    if (!button || button.disabled) return false;
    button.click();
    return true;
  }, labels);
};

const panelText = () =>
  run(() => document.querySelector('[role="tabpanel"][data-state="active"]')?.innerText?.trim() ?? "");

const emptyState = await panelText();
let started = false;
let startError = "";
for (let attempt = 0; attempt < 6 && !started; attempt++) {
  const showing = await panelText();
  started = showing.includes("正在监控") || showing.includes("Watching");
  if (started) break;
  if (!(await press(["Start", "开始"]))) {
    await wait(1500);
    continue;
  }
  await wait(1800);
  const text = await panelText();
  started = text.includes("正在监控") || text.includes("Watching");
  startError = await run(() => document.querySelector('[role="alert"]')?.textContent?.trim() ?? "");
  if (!started) await wait(1500);
}
if (!started) throw new Error(`could not start a session: ${startError} :: ${(await panelText()).slice(0, 240)}`);
await wait(3500);
const during = await panelText();
if (!(await press(["Stop", "停止"]))) throw new Error("stop button did not become enabled");
await wait(1500);
const afterStop = await panelText();

const rows = await run(() => document.querySelectorAll('.rows > .row[role="button"]').length);
if (rows === 0) problems.push("no session row after stopping");
await run(() => document.querySelector('.rows > .row[role="button"]')?.click());
await wait(1200);
const report = await run(() => {
  const panel = document.querySelector('[role="tabpanel"][data-state="active"]');
  return {
    text: (panel?.innerText ?? "").trim(),
    charts: panel?.querySelectorAll("svg.chart").length ?? 0,
    cards: panel?.querySelectorAll("section").length ?? 0,
  };
});
if (!report.text || report.charts < 8) problems.push(`report incomplete: ${report.charts} charts`);

const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync("shots/perf-report.png", Buffer.from(shot.result.data, "base64"));

console.log(
  JSON.stringify(
    {
      tab,
      empty: emptyState.slice(0, 220),
      during: during.slice(0, 320),
      afterStop: afterStop.slice(0, 320),
      rows,
      report: { ...report, text: report.text.slice(0, 900) },
      problems,
    },
    null,
    2,
  ),
);
socket.close();
if (problems.length) process.exit(1);
