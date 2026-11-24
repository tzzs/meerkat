// Verifies the tray panel's lower chrome is actually on screen, not clipped by a window
// that was sized from an already-shrunk flex card. Needs a Meerkat debug build started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9334.
//
//   MEERKAT_CDP=9334 node scripts/panel-probe.mjs [shot.png]
import { writeFileSync } from "node:fs";

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
const page = targets.find((t) => t.type === "page" && t.url.includes("panel.html"));
if (!page) {
  console.log("no panel page", targets.map((t) => t.url).join(", "));
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
const run = async (fn) => {
  const response = await send("Runtime.evaluate", {
    expression: `(${fn.toString()})()`,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.result?.exceptionDetails) {
    throw new Error(String(response.result.exceptionDetails.exception?.description ?? "evaluate failed"));
  }
  return response.result?.result?.value;
};

await send("Runtime.enable");
// A plain browser uses the whole window for its viewport. The real panel is a 400x580
// logical window; set that when asked so the flex layout is measured in the shape it ships in.
if (process.env.PANEL_VIEWPORT) {
  const [width, height] = process.env.PANEL_VIEWPORT.split("x").map(Number);
  await send("Emulation.setDeviceMetricsOverride", {
    width: width || 400,
    height: height || 580,
    deviceScaleFactor: Number(process.env.PANEL_DPR) || 1.75,
    mobile: false,
  });
}
let measured = null;
for (let attempt = 0; attempt < 20 && !measured; attempt++) {
  measured = await run(() => {
    const panel = document.querySelector(".panel");
    if (!panel || !document.querySelector(".panel-tabs button")) return null;
    const section = (selector) => {
      const node = panel.querySelector(selector);
      return node ? { height: node.offsetHeight, content: node.scrollHeight } : null;
    };
    const row = panel.querySelector(".panel-app");
    return {
      window: { width: innerWidth, height: innerHeight },
      card: { height: panel.offsetHeight, content: panel.scrollHeight },
      head: section(".panel-head"),
      metrics: section(".panel-metrics"),
      apps: section(".panel-apps"),
      tabs: section(".panel-tabs"),
      rowHeight: row?.offsetHeight ?? 0,
      tabCount: panel.querySelectorAll(".panel-tabs button").length,
      tabLabels: [...panel.querySelectorAll(".panel-tabs button")].map((b) => b.textContent.trim()),
    };
  });
  if (!measured) await wait(250);
}
if (!measured) {
  console.log("the panel did not render");
  socket.close();
  process.exit(1);
}

const problems = [];
if (measured.tabCount === 0) problems.push("no tab strip");
if (measured.tabs && measured.tabs.height < measured.tabs.content - 1) {
  problems.push(`tab strip clipped: ${measured.tabs.height}px for ${measured.tabs.content}px`);
}
if (measured.apps && measured.apps.height < measured.rowHeight - 1) {
  problems.push(`app rows clipped: ${measured.apps.height}px for a ${measured.rowHeight}px row`);
}
if (measured.card.height > measured.window.height - 24 + 1) {
  problems.push(`card taller than its window's margins: ${measured.card.height}px`);
}

if (process.argv[2]) {
  const shot = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(process.argv[2], Buffer.from(shot.result.data, "base64"));
}
console.log(JSON.stringify({ ...measured, problems }, null, 2));
socket.close();
if (problems.length) process.exit(1);
