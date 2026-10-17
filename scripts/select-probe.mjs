// Checks the settings dropdowns that became a Radix Select: what the trigger is wearing, what
// the list is wearing (the whole reason for the change — an OS popup cannot take the app's
// theme), and whether a choice made with the keyboard actually reaches the settings store.
//
//   MEERKAT_CDP=9334 node scripts/select-probe.mjs
//
// It changes the theme twice and puts it back, so run it against the dev window only.
import { mkdirSync, writeFileSync } from "node:fs";

const PORT = process.env.MEERKAT_CDP ?? "9334";
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
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const run = async (fn, ...args) => {
  const expr = `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(",")})`;
  const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true });
  if (r.result?.exceptionDetails) {
    return `EXC ${r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails.text}`;
  }
  return r.result?.result?.value;
};
const press = async (key) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key });
  await wait(250);
};

await send("Page.enable");
await send("Page.reload", { ignoreCache: true });
await wait(3000);

const clickText = (selector, text) => {
  const b = [...document.querySelectorAll(selector)].find((n) => n.textContent.trim() === text);
  if (b) {
    b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    b.click();
  }
  return Boolean(b);
};

console.log("gear:", await run(clickText, "button", "⚙"));
await wait(900);

// The trigger, measured against the CSS that used to style the native select.
console.log(
  "trigger:",
  JSON.stringify(
    await run((label) => {
      const row = [...document.querySelectorAll(".row-setting")].find((n) =>
        n.textContent.includes(label),
      );
      const t = row?.querySelector('[role="combobox"], button');
      if (!t) return `no trigger in the ${label} row`;
      const s = getComputedStyle(t);
      return {
        label: row.querySelector(".row-label").textContent.trim(),
        name: t.getAttribute("aria-label"),
        labelsFromRow: t.labels?.length ?? 0,
        labelledByRow: t.textContent.trim(),
        role: t.getAttribute("role"),
        expanded: t.getAttribute("aria-expanded"),
        background: s.backgroundColor,
        border: `${s.borderTopWidth} ${s.borderTopColor}`,
        radius: s.borderTopLeftRadius,
        padding: `${s.paddingTop} ${s.paddingLeft}`,
        minWidth: s.minWidth,
        height: Math.round(t.getBoundingClientRect().height),
        width: Math.round(t.getBoundingClientRect().width),
        fontSize: s.fontSize,
      };
    }, "外观"),
  ),
);

const themeBefore = await run(() => document.documentElement.getAttribute("data-theme"));
console.log("theme before:", JSON.stringify(themeBefore));

// Open it and look at the list: whose colours is it wearing, and is the chosen one marked?
await run(() => {
  const row = [...document.querySelectorAll(".row-setting")].find((n) => n.textContent.includes("外观"));
  row.querySelector('[role="combobox"]').click();
  return true;
});
await wait(500);
console.log(
  "listbox:",
  JSON.stringify(
    await run(() => {
      const list = document.querySelector('[role="listbox"]');
      if (!list) return "no listbox";
      const s = getComputedStyle(list);
      return {
        inBody: list.closest(".settings") === null,
        popupBackground: s.backgroundColor,
        popupBorder: `${s.borderTopWidth} ${s.borderTopColor}`,
        radius: s.borderTopLeftRadius,
        shadow: s.boxShadow.slice(0, 40),
        options: [...list.querySelectorAll('[role="option"]')].map((o) => ({
          text: o.textContent.trim(),
          selected: o.getAttribute("aria-selected"),
          highlighted: o.dataset.highlighted,
          check: Boolean(o.querySelector("svg:last-child")),
        })),
      };
    }),
  ),
);
const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync("shots/select-open.png", Buffer.from(shot.result.data, "base64"));
console.log("wrote shots/select-open.png");

// Choose with the keyboard: down to the next option, Enter to commit.
await press("ArrowDown");
await press("Enter");
console.log(
  "after ArrowDown+Enter:",
  JSON.stringify(await run(() => ({
    theme: document.documentElement.getAttribute("data-theme"),
    trigger: [...document.querySelectorAll(".row-setting")]
      .find((n) => n.textContent.includes("外观"))
      ?.querySelector('[role="combobox"]')?.textContent?.trim(),
    open: Boolean(document.querySelector('[role="listbox"]')),
  }))),
);

// Put the theme back exactly as it was, then check the compact tray dropdown.
await run(() => {
  const row = [...document.querySelectorAll(".row-setting")].find((n) => n.textContent.includes("外观"));
  row.querySelector('[role="combobox"]').click();
  return true;
});
await wait(400);
const back = await run(
  (wanted) => {
    const list = document.querySelector('[role="listbox"]');
    const target = [...list.querySelectorAll('[role="option"]')].find(
      (o) => (o.dataset.value ?? "") === wanted || o.textContent.trim() === wanted,
    );
    target?.click();
    return { wanted, found: Boolean(target) };
  },
  themeBefore === null ? "跟随 Windows" : themeBefore === "dark" ? "深色" : "浅色",
);
await wait(500);
console.log(
  "restored:",
  JSON.stringify(back),
  "theme now",
  JSON.stringify(await run(() => document.documentElement.getAttribute("data-theme"))),
);

console.log("layout page:", await run(clickText, "button", "布局"));
await wait(800);
console.log(
  "tray dropdown:",
  JSON.stringify(
    await run(() => {
      const t = document.querySelector(".tray-row [role='combobox']");
      if (!t) return "no tray combobox";
      const s = getComputedStyle(t);
      return {
        name: t.getAttribute("aria-label"),
        padding: `${s.paddingTop} ${s.paddingLeft}`,
        radius: s.borderTopLeftRadius,
        fontSize: s.fontSize,
        height: Math.round(t.getBoundingClientRect().height),
        width: Math.round(t.getBoundingClientRect().width),
      };
    }),
  ),
);

// The open-window-on list is the one that could repeat a value; Radix refuses duplicates, so a
// rendered list with no exception means the ids are unique.
console.log(
  "openWindowOn options:",
  JSON.stringify(
    await run(() => {
      const row = [...document.querySelectorAll(".row-setting")].find((n) =>
        n.textContent.includes("窗口打开时停在"),
      );
      if (!row) return "row not found";
      row.querySelector('[role="combobox"]').click();
      return row.querySelector('[role="combobox"]').textContent.trim();
    }),
  ),
);
await wait(400);
console.log(
  "  in the list:",
  JSON.stringify(
    await run(() => {
      const list = document.querySelector('[role="listbox"]');
      const values = list ? [...list.querySelectorAll('[role="option"]')].map((o) => o.textContent.trim()) : null;
      document.body.click();
      return values;
    }),
  ),
);
await press("Escape");
await wait(300);
console.log("closed again:", await run(() => !document.querySelector('[role="listbox"]')));
process.exit(0);
