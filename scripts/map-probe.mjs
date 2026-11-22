// Reads the memory map out of the running window and takes the acceptance shots: where the
// card sits, how many boxes it drew, whether any is a sliver, that a box opens the app detail,
// and the same map in the dark theme.
//
//   MEERKAT_CDP=9334 node scripts/map-probe.mjs shots/map
import { writeFileSync } from "node:fs";

const PORT = process.env.MEERKAT_CDP ?? "9334";
const base = process.argv[2] ?? "shots/map";
const lightPath = `${base}-light.png`;
const darkPath = `${base}-dark.png`;

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
  if (r.result?.exceptionDetails) return `EXCEPTION ${r.result.exceptionDetails.text}`;
  return r.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const openMemory = () =>
  evaluate(`(() => {
    const b = [...document.querySelectorAll('[role="tab"]')].find((n) => /内存|Memory/.test(n.textContent.trim()));
    if (!b) return false;
    b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    b.click();
    return true;
  })()`);
const activeTab = () =>
  evaluate(`document.querySelector('[role="tab"][data-state="active"]')?.textContent?.trim() ?? ""`);
const shoot = async (path) => {
  const tab = await activeTab();
  const shot = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(path, Buffer.from(shot.result.data, "base64"));
  console.log(`wrote ${path} at ${tab}`);
  return tab;
};

await send("Page.enable");
await send("Runtime.enable");

// A detail sheet left open by another run would eat the first shot.
await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await wait(600);

// The window can be showing another tab (the app remembers where it was), so the map is
// checked for rather than assumed.
if (!(await openMemory())) console.log("no Memory tab found");
await wait(1200);
if (!(await evaluate(`Boolean(document.querySelector(".memory-map"))`))) await openMemory();
await wait(800);

const read = () =>
  evaluate(`(() => {
    const panel = document.querySelector('[role="tabpanel"][data-state="active"]');
    const cards = [...(panel?.querySelectorAll("section") ?? [])].map((s) => ({
      title: s.querySelector("h3")?.textContent?.trim() ?? "",
      top: Math.round(s.getBoundingClientRect().top + scrollY),
    }));
    const map = document.querySelector(".memory-map");
    const boxes = [...(map?.querySelectorAll(".mem-box") ?? [])].map((b) => {
      const r = b.getBoundingClientRect();
      return {
        label: b.querySelector(".map-box-name")?.textContent?.trim() ?? "",
        size: b.querySelector(".map-box-size")?.textContent?.trim() ?? "",
        w: Math.round(r.width),
        h: Math.round(r.height),
        clickable: b.tagName === "BUTTON",
      };
    });
    return {
      tab: document.querySelector('[role="tab"][data-state="active"]')?.textContent?.trim(),
      mapCardIndex: cards.findIndex((c) => c.title === map?.closest("section")?.querySelector("h3")?.textContent?.trim()),
      cards,
      hasMap: Boolean(map),
      boxCount: boxes.length,
      named: boxes.filter((b) => b.label).length,
      slivers: boxes.filter((b) => Math.min(b.w, b.h) < 12).length,
      unlabelled: boxes.filter((b) => !b.label).length,
      tail: boxes.find((b) => b.label && /其余|Everything/.test(b.label)) ?? null,
      boxes: boxes.slice(0, 6),
      caption: map?.parentElement?.querySelector(".hint")?.textContent?.trim() ?? "",
    };
  })()`);

console.log(JSON.stringify(await read(), null, 2));

await evaluate(`document.querySelector(".memory-map")?.scrollIntoView({ block: "center" }); true`);
await wait(900);
await shoot(lightPath);

const clicked = await evaluate(`(() => {
  const box = [...document.querySelectorAll(".mem-box")].find((b) => b.tagName === "BUTTON" && b.querySelector(".map-box-name"));
  if (!box) return "";
  const name = box.querySelector(".map-box-name").textContent.trim();
  box.click();
  return name;
})()`);
await wait(1000);
const dialog = await evaluate(`document.querySelector('[role="dialog"]')?.textContent?.slice(0, 80) ?? ""`);
console.log("clicked", JSON.stringify(clicked), "| detail sheet:", dialog ? "open" : "MISSING");
await evaluate(`document.querySelector('[role="dialog"] [aria-label], [role="dialog"] button')?.click(); true`);
await wait(600);

await evaluate(`document.documentElement.setAttribute("data-theme", "dark"); true`);
await wait(600);
await shoot(darkPath);
console.log("dark box fill:", await evaluate(`getComputedStyle(document.querySelector(".mem-box")).backgroundColor`));
await evaluate(`document.documentElement.removeAttribute("data-theme"); true`);

socket.close();
