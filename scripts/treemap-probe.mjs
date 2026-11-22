// Reads an app map out of the running window and takes the acceptance shots: where the card
// sits, how many boxes it drew, whether any is a sliver, that a box opens the app detail, that
// the folded box drills into its own map and comes back, and the same map in the dark theme.
//
//   MEERKAT_CDP=9334 node scripts/treemap-probe.mjs shots/map 内存
//   MEERKAT_CDP=9334 node scripts/treemap-probe.mjs shots/cpu 处理器
import { writeFileSync } from "node:fs";

const PORT = process.env.MEERKAT_CDP ?? "9334";
const [base = "shots/map", label = "内存"] = process.argv.slice(2);
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

const openTab = () =>
  evaluate(`(() => {
    const b = [...document.querySelectorAll('[role="tab"]')].find((n) => n.textContent.trim().includes(${JSON.stringify(label)}));
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

// CDP answers before the window's first paint on a cold start, so the tab strip is waited for.
for (let i = 0; i < 40; i++) {
  if (await evaluate(`document.querySelectorAll('[role="tab"]').length > 0`)) break;
  await wait(500);
}

// A detail sheet left open by another run would eat the first shot.
await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await wait(600);

// The window can be showing another tab (the app remembers where it was), so the map is
// checked for rather than assumed.
if (!(await openTab())) console.log(`no ${label} tab found`);
await wait(1200);
if (!(await evaluate(`Boolean(document.querySelector(".app-map"))`))) await openTab();
await wait(800);

const read = () =>
  evaluate(`(() => {
    const panel = document.querySelector('[role="tabpanel"][data-state="active"]');
    const cards = [...(panel?.querySelectorAll("section") ?? [])].map((s) => ({
      title: s.querySelector("h3")?.textContent?.trim() ?? "",
      top: Math.round(s.getBoundingClientRect().top + scrollY),
    }));
    const map = document.querySelector(".app-map");
    const boxes = [...(map?.querySelectorAll(".treemap-box") ?? [])].map((b) => {
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

const readLevel = () =>
  evaluate(`(() => {
    const map = document.querySelector(".app-map");
    const trail = document.querySelector(".app-map-trail");
    const other = map?.querySelector(".treemap-box.other");
    return {
      boxCount: map?.querySelectorAll(".treemap-box").length ?? 0,
      back: Boolean(trail?.querySelector("button")),
      help: map?.parentElement?.querySelector(".hint")?.textContent?.trim() ?? "",
      other: other
        ? {
            text: other.textContent?.replace(/\\s+/g, " ").trim() ?? "",
            enterHint: other.querySelector(".map-box-enter")?.textContent?.trim() ?? "",
          }
        : null,
    };
  })()`);

const root = await read();
console.log(JSON.stringify(root, null, 2));

await evaluate(`document.querySelector(".app-map")?.scrollIntoView({ block: "center" }); true`);
await wait(900);
await shoot(lightPath);

const clicked = await evaluate(`(() => {
  const box = [...document.querySelectorAll(".treemap-box")].find((b) => b.tagName === "BUTTON" && b.querySelector(".map-box-name"));
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

const entered = await evaluate(`(() => {
  const other = document.querySelector(".treemap-box.other");
  if (!other) return "";
  const size = other.querySelector(".map-box-size")?.textContent?.trim() ?? "";
  other.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  // A box too short for its size line still counts as opened.
  return size || "opened";
})()`);

if (!entered) {
  console.log("no other box on the map, so the drill was not exercised");
} else {
  await wait(900);
  const levelOne = await readLevel();
  console.log("opened the other box", JSON.stringify(entered));
  console.log("levelOne", JSON.stringify(levelOne, null, 2));
  await shoot(`${base}-level.png`);

  if (levelOne.other) {
    await evaluate(
      `document.querySelector(".treemap-box.other").dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true })); true`,
    );
    await wait(900);
    const levelTwo = await readLevel();
    console.log("levelTwo", JSON.stringify({ boxCount: levelTwo.boxCount, help: levelTwo.help, other: Boolean(levelTwo.other) }));
  }

  for (let i = 0; i < 3; i++) {
    if (!(await evaluate(`Boolean(document.querySelector(".app-map-trail button"))`))) break;
    await evaluate(`document.querySelector(".app-map-trail button").click(); true`);
    await wait(600);
  }
  const returned = await evaluate(`(() => {
    const other = document.querySelector(".treemap-box.other");
    return {
      trail: Boolean(document.querySelector(".app-map-trail")),
      text: other?.textContent?.replace(/\\s+/g, " ").trim() ?? "",
    };
  })()`);
  const rootLabel = root.tail?.label ?? "";
  console.log(
    "back at root:",
    returned.trail ? "TRAIL STILL OPEN" : "trail closed",
    "| other box:",
    rootLabel && returned.text.includes(rootLabel) ? "restored" : "MISSING",
  );
}

await evaluate(`document.documentElement.setAttribute("data-theme", "dark"); true`);
await wait(600);
await shoot(darkPath);
console.log("dark box fill:", await evaluate(`getComputedStyle(document.querySelector(".treemap-box")).backgroundColor`));
await evaluate(`document.documentElement.removeAttribute("data-theme"); true`);

socket.close();
