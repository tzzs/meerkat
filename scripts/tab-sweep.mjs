// Walks every tab in the running window and reports what a regression would show: a blank
// panel, a thrown error, a list that stopped measuring itself, a card count that changed.
//
//   PULSE_CDP=9334 node scripts/tab-sweep.mjs
//   PULSE_CDP=9334 node scripts/tab-sweep.mjs "概览 处理器 内存"
//
// Screenshots land in shots/sweep-<tab>.png. It only ever clicks tabs and reads, so it is safe
// to run against the window you are looking at.
import { mkdirSync, writeFileSync } from "node:fs";

const PORT = process.env.PULSE_CDP ?? "9334";
const only = (process.argv[2] ?? "").split(" ").filter(Boolean);

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
const problems = [];
socket.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
  if (msg.method === "Runtime.exceptionThrown") {
    problems.push(`throw: ${msg.params.exceptionDetails.exception?.description?.slice(0, 120)}`);
  }
});
const send = (method, params = {}) => {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
};
const run = async (fn, ...args) => {
  const expr = `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(",")})`;
  const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true });
  if (r.result?.exceptionDetails) return `EXC ${r.result.exceptionDetails.exception?.description?.slice(0, 120)}`;
  return r.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send("Page.enable");
await send("Runtime.enable");
await send("Page.reload", { ignoreCache: true });
await wait(3000);

const tabs = await run(() => [...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent.trim()));
if (!tabs.length) {
  // A sweep that walks nothing finds nothing to complain about. This printed "no problems
  // found" while the window was showing Edge's connection-refused page, which is the one
  // thing a regression harness must never do.
  const why = await run(() => `${document.title} :: ${document.body.innerText.trim().slice(0, 120)}`);
  console.log(`no tabs at all — the window is not showing the app.\n  ${why}`);
  process.exit(1);
}
const only2 = only.filter((t) => tabs.includes(t));
if (only.length && !only2.length) {
  console.log(`none of the requested tabs are on screen; found: ${tabs.join(" ")}`);
  process.exit(1);
}
const walk = only2.length ? only2 : tabs;
console.log("tabs:", walk.join(" "));

const read = () => {
  const active = document.querySelector('[role="tab"][data-state="active"]')?.textContent?.trim();
  const panel = document.querySelector('[role="tabpanel"][data-state="active"]');
  const tables = [...panel?.querySelectorAll(".app-table .rows") ?? []];
  return {
    active,
    cards: panel?.querySelectorAll("section").length ?? 0,
    text: (panel?.innerText ?? "").trim().length,
    tables: tables.map((rows) => ({
      mounted: rows.querySelectorAll(".row").length,
      heights: [...new Set([...rows.querySelectorAll(".row")].map((r) => Math.round(r.getBoundingClientRect().height)))].join("/"),
      // The list is not its own scroller: the page scrolls and the virtualizer is told the
      // offset with scrollMargin, so asking the list whether it scrolls is asking the wrong node.
      total: Math.round(rows.getBoundingClientRect().height),
      count: Number(rows.dataset.count ?? 0),
    })),
    charts: panel?.querySelectorAll("svg.chart").length ?? 0,
    sliders: panel?.querySelectorAll('[role="slider"]').length ?? 0,
    pickers: panel?.querySelectorAll('[role="radiogroup"]').length ?? 0,
  };
};

for (const label of walk) {
  await run((text) => {
    const b = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim() === text);
    if (b) {
      b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
      b.click();
    }
    return Boolean(b);
  }, label);
  await wait(1400);
  const state = await run(read);
  console.log(JSON.stringify(state));
  if (!state.active || state.text < 40) problems.push(`${label}: panel looks empty (${state.text} chars)`);
  for (const table of state.tables ?? []) {
    if (table.mounted === 0) problems.push(`${label}: a table mounted no rows`);
    if (table.mounted > 40) problems.push(`${label}: a table mounted ${table.mounted} rows, virtualization may be off`);
  }
  const shot = await send("Page.captureScreenshot", { format: "png" });
  mkdirSync("shots", { recursive: true });
  writeFileSync(`shots/sweep-${label}.png`, Buffer.from(shot.result.data, "base64"));
}

// Scroll the busiest table to the bottom and check the last app is reachable. The list lives
// on the tabs with an app table, so go back to one rather than testing the last tab visited.
await run((text) => {
  const b = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim() === text);
  if (b) { b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true })); b.click(); }
  return Boolean(b);
}, ["内存", "处理器", "磁盘", "网络", "概览"].find((t) => walk.includes(t)) ?? walk[0]);
await wait(1400);
const scrolled = await run(() => {
  const scroller = document.querySelector(".content");
  if (!scroller) return null;
  scroller.scrollTop = scroller.scrollHeight;
  return { scrollHeight: scroller.scrollHeight, clientHeight: scroller.clientHeight, scrollTop: Math.round(scroller.scrollTop) };
});
if (scrolled) {
  await wait(700);
  const after = await run(() => {
    const rows = [...document.querySelectorAll(".app-table .rows")].sort((a, b) => b.scrollHeight - a.scrollHeight)[0];
    const mounted = rows.querySelectorAll(".row").length;
    const last = rows.lastElementChild;
    const bottom = [...rows.querySelectorAll(".row")].reduce((m, r) => Math.max(m, Number(r.dataset.index ?? 0)), 0);
    return {
      mounted,
      count: Number(rows.dataset.count ?? 0),
      lastIndexMounted: bottom,
      lastVisible: last ? Math.round(last.getBoundingClientRect().bottom) <= innerHeight + 2 : false,
      names: [...rows.querySelectorAll(".row-name")].slice(-2).map((n) => n.textContent.trim()),
    };
  });
  console.log("scrolled to the end:", JSON.stringify({ ...scrolled, ...after }));
  if (after.count > after.mounted && after.lastIndexMounted < after.count - 1) {
    problems.push(`scrolling to the bottom mounted up to row ${after.lastIndexMounted} of ${after.count}`);
  }
}

console.log(problems.length ? `PROBLEMS:\n- ${problems.join("\n- ")}` : "no problems found");
process.exit(problems.length ? 1 : 0);
