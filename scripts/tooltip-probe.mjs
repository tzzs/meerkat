// Checks the tooltips that replaced the `title` attribute, in both windows: whether one opens,
// what it is wearing, and whether the row it hangs off still measures what the virtualizer
// thinks it measures.
//
//   PULSE_CDP=9334 node scripts/tooltip-probe.mjs
import { mkdirSync, writeFileSync } from "node:fs";

const PORT = process.env.PULSE_CDP ?? "9334";

async function connect(wantPanel) {
  const targets = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json());
  const page = targets.find((t) => t.type === "page" && t.url.includes("panel") === wantPanel);
  if (!page) throw new Error(`no ${wantPanel ? "panel" : "main"} target`);
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
  const run = async (fn, ...args) => {
    const expr = `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(",")})`;
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true });
    if (r.result?.exceptionDetails) {
      return `EXC ${r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails.text}`;
    }
    return r.result?.result?.value;
  };
  const moveTo = async (x, y) => {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await new Promise((resolve) => setTimeout(resolve, 120));
  };
  // A real pointer has to leave the trigger for Radix to close what it opened; a synthetic
  // pointerleave on the body does nothing, and the next read then sees the previous tooltip.
  const away = () => moveTo(600, 600);
  const hover = async (fn, ...args) => {
    await away();
    const where = await run(fn, ...args);
    if (!where) return null;
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: where.x, y: where.y });
    await new Promise((resolve) => setTimeout(resolve, where.wait ?? 600));
    return where;
  };
  const readTip = () =>
    run(() => {
      const tip = document.querySelector('[role="tooltip"]');
      if (!tip) return null;
      const s = getComputedStyle(tip);
      const r = tip.getBoundingClientRect();
      return {
        text: tip.textContent.trim(),
        background: s.backgroundColor,
        border: `${s.borderTopWidth} ${s.borderTopColor}`,
        radius: s.borderTopLeftRadius,
        fontSize: s.fontSize,
        color: s.color,
        whiteSpace: s.whiteSpace,
        inBody: tip.closest(".shell, .panel-body") === null,
        top: Math.round(r.top),
        left: Math.round(r.left),
      };
    });
  return { send, run, hover, readTip, away, url: page.url, socket };
}

const main = await connect(false);
await main.send("Page.enable");
await main.send("Page.reload", { ignoreCache: true });
await new Promise((r) => setTimeout(r, 3000));

const clickTab = (label) => {
  const b = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim() === label);
  if (b) {
    b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    b.click();
  }
  return Boolean(b);
};

console.log("no tooltip while the pointer is still:", JSON.stringify(await main.readTip()));

// A window control: the delay is the provider's, so nothing should show for 350ms and then it
// should.
await main.hover(() => {
  const b = document.querySelector(".winctl");
  const r = b.getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
});
console.log("window control:", JSON.stringify(await main.readTip()));
await main.away();
console.log("  gone once the pointer leaves:", JSON.stringify(await main.readTip()));

// The core grid answers at once, because sweeping the pointer across twenty bars is one task.
await main.run(clickTab, "处理器");
await new Promise((r) => setTimeout(r, 1200));
const core = await main.hover(
  (n) => {
    const cell = document.querySelectorAll(".core")[n];
    if (!cell) return null;
    const r = cell.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, wait: 120 };
  },
  3,
);
console.log("core cell 3 after 120ms:", JSON.stringify(await main.readTip()), JSON.stringify(core));

// An app row: the long one, with window titles.
await main.run(clickTab, "内存");
await new Promise((r) => setTimeout(r, 1200));
const row = await main.hover(() => {
  const r0 = document.querySelector(".app-table .rows .row");
  if (!r0) return null;
  // A row in a virtualized list is usually below the fold, and a pointer moved to a point
  // outside the window simply never arrives.
  r0.scrollIntoView({ block: "center" });
  return null;
});
await new Promise((resolve) => setTimeout(resolve, 400));
const rowPoint = await main.run(() => {
  const rows = document.querySelector(".app-table .rows");
  const r0 = [...rows.querySelectorAll(".row")].find((r) => {
    const b = r.getBoundingClientRect();
    return b.top > 80 && b.bottom < innerHeight - 20;
  });
  if (!r0) return null;
  const box = r0.getBoundingClientRect();
  return { x: box.x + 120, y: box.y + box.height / 2, name: r0.querySelector(".row-name")?.textContent };
});
await main.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: rowPoint.x, y: rowPoint.y });
await new Promise((resolve) => setTimeout(resolve, 700));
console.log("app row", JSON.stringify(rowPoint?.name), "tooltip:", JSON.stringify(await main.readTip()));
console.log(
  "  the row still measures itself:",
  JSON.stringify(
    await main.run(() => {
      const rows = document.querySelector(".app-table .rows");
      const mounted = rows.querySelectorAll(".row").length;
      const heights = [...new Set([...rows.querySelectorAll(".row")].map((r) => Math.round(r.getBoundingClientRect().height)))];
      return { mounted, heights, scrollTop: Math.round(rows.scrollTop) };
    }),
  ),
);
const shot = await main.send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync("shots/tooltip-row.png", Buffer.from(shot.result.data, "base64"));
console.log("wrote shots/tooltip-row.png");

// The tray panel is its own window and its own provider.
try {
  const panel = await connect(true);
  await panel.send("Page.enable");
  await panel.run(() => {
    const b = [...document.querySelectorAll(".panel-tabs button")].find((n) => n.textContent.trim() === "概览");
    b?.click();
    return Boolean(b);
  });
  await new Promise((r) => setTimeout(r, 1500));
  await panel.hover(() => {
    const b = document.querySelector(".icon-btn");
    const r = b.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
  });
  console.log("panel pin:", JSON.stringify(await panel.readTip()));
  await panel.run(() => document.body.click());
  await new Promise((r) => setTimeout(r, 400));
  await panel.hover(() => {
    const r0 = document.querySelector(".panel-app");
    if (!r0) return null;
    const box = r0.getBoundingClientRect();
    return { x: box.x + 60, y: box.y + box.height / 2, wait: 700 };
  });
  console.log("panel app row:", JSON.stringify(await panel.readTip()));
  const pshot = await panel.send("Page.captureScreenshot", { format: "png" });
  writeFileSync("shots/tooltip-panel.png", Buffer.from(pshot.result.data, "base64"));
  console.log("wrote shots/tooltip-panel.png");
} catch (error) {
  console.log("panel:", String(error).slice(0, 80));
}

// Leave the window where the user expects it.
await main.run(clickTab, "概览");
await new Promise((r) => setTimeout(r, 600));
process.exit(0);
