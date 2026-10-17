// Checks the chip rows that became a ToggleGroup: the range picker on every chart, the sort
// picker on the app table, and the panel-tile picker in Settings. Reports what the group says
// it is, what the chosen chip is wearing, and whether the keyboard can move the choice.
//
//   MEERKAT_CDP=9334 node scripts/chip-probe.mjs
//
// The page functions below are written as real functions and sent by source (`fn.toString()`),
// because an expression assembled by string interpolation is a syntax error you can only find
// in the browser.
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
  if (r.result?.exceptionDetails) return `EXCEPTION ${JSON.stringify(r.result.exceptionDetails)}`;
  return r.result?.result?.value;
};
const press = async (key) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key });
  await wait(300);
};
const clickBy = async (matcher, ...args) => {
  const found = await run(matcher, ...args);
  await wait(900);
  return found;
};

await send("Page.enable");
// Always reload: an HMR update that did not take leaves the probe reading the previous
// component, and the failure looks like a bug in the new one.
await send("Page.reload", { ignoreCache: true });
await wait(3000);

const clickTab = (label) => {
  const b = [...document.querySelectorAll('[role="tab"], .tab')].find((n) => n.textContent.trim() === label);
  if (b) {
    b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    b.click();
  }
  return Boolean(b);
};

const clickText = (selector, text) => {
  const b = [...document.querySelectorAll(selector)].find((n) => n.textContent.trim() === text);
  if (b) {
    b.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    b.click();
  }
  return Boolean(b);
};

// The group's own semantics, the gap the deleted CSS used to set, and what each chip looks like
// chosen versus not. Every page has more than one radiogroup — each chart carries its own range
// picker — so a group is named by its accessible label, not by which one happens to be first.
const readGroup = (role, label) => {
  const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
    (n) => n.getAttribute("aria-label") === label,
  );
  if (!g) return `no [role=${role}] labelled ${label}`;
  const style = getComputedStyle(g);
  // Both kinds of item carry data-state; only the single ones carry role="radio", because a
  // native button already is a button as far as ARIA is concerned.
  const items = [...g.querySelectorAll("[data-state]")];
  return {
    label: g.getAttribute("aria-label"),
    gap: style.columnGap,
    wrap: style.flexWrap,
    chips: items.map((el) => {
      const s = getComputedStyle(el);
      return {
        text: el.textContent.trim(),
        state: el.dataset.state,
        checked: el.getAttribute("aria-checked") ?? el.getAttribute("aria-pressed"),
        tabIndex: el.tabIndex,
        background: s.backgroundColor,
        border: `${s.borderTopWidth} ${s.borderTopColor}`,
        weight: s.fontWeight,
        radius: s.borderTopLeftRadius,
        padding: `${s.paddingTop} ${s.paddingLeft}`,
        x: Math.round(el.getBoundingClientRect().x),
        width: Math.round(el.getBoundingClientRect().width),
      };
    }),
  };
};

console.log("back to overview:", await clickBy(clickTab, "概览"));
console.log("range group:", JSON.stringify(await run(readGroup, "radiogroup", "时间范围"), null, 1));

await run(
  (role, label) => {
    const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
      (n) => n.getAttribute("aria-label") === label,
    );
    const on = g?.querySelector('[data-state="on"]');
    if (on) on.focus();
    return document.activeElement === on;
  },
  "radiogroup",
  "时间范围",
);
await press("ArrowRight");
console.log(
  "after ArrowRight:",
  JSON.stringify(
    await run(
      (role, label) => {
        const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
          (n) => n.getAttribute("aria-label") === label,
        );
        const on = g.querySelector('[data-state="on"]');
        return {
          chosen: on?.textContent?.trim(),
          focusIsTheChosenOne: document.activeElement === on,
          rovingTabIndex: [...g.children].map((c) => c.tabIndex).join(","),
          chartTitle: document.querySelector(".card-head .num, .stat-value")?.textContent?.trim(),
        };
      },
      "radiogroup",
      "时间范围",
    ),
  ),
);
await press("ArrowLeft");
console.log(
  "after ArrowLeft :",
  JSON.stringify(
    await run(
      (role, label) => {
        const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
          (n) => n.getAttribute("aria-label") === label,
        );
        return { chosen: g.querySelector('[data-state="on"]')?.textContent?.trim() };
      },
      "radiogroup",
      "时间范围",
    ),
  ),
);

console.log("to memory:", await clickBy(clickTab, "内存"));
const sort = await run(readGroup, "radiogroup", "排序");
console.log("sort group:", JSON.stringify(sort, null, 1));
console.log(
  "sort picker right-aligned?",
  JSON.stringify(
    await run(
      (role, label) => {
        const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
          (n) => n.getAttribute("aria-label") === label,
        );
        const row = g.closest(".table-tools");
        return {
          gapToRowRight: Math.round(row.getBoundingClientRect().right - g.getBoundingClientRect().right),
        };
      },
      "radiogroup",
      "排序",
    ),
  ),
);
// Choose a different sort and read the table back, then put it where it was.
for (const label of ["磁盘", "内存"]) {
  console.log(
    `click ${label}:`,
    JSON.stringify(
      await clickBy(
        (role, groupLabel, chip) => {
          const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
            (n) => n.getAttribute("aria-label") === groupLabel,
          );
          [...g.querySelectorAll("[data-state]")].find((n) => n.textContent.trim() === chip)?.click();
          return g.querySelector('[data-state="on"]')?.textContent?.trim();
        },
        "radiogroup",
        "排序",
        label,
      ),
    ),
  );
}

console.log("gear:", await clickBy(clickText, "button", "⚙"));
console.log("layout page:", await clickBy(clickText, "button", "布局"));
console.log("tiles group:", JSON.stringify(await run(readGroup, "toolbar", "面板中显示的卡片"), null, 1));
// Toggle one tile off and on again, and confirm the panel layout really changed.
const tileState = (role, label) => {
  const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
    (n) => n.getAttribute("aria-label") === label,
  );
  return [...g.querySelectorAll('[data-state="on"]')].map((n) => n.textContent.trim()).join("/");
};
for (const step of ["before", "after one click", "after the second"]) {
  if (step !== "before") {
    await clickBy(
      (role, label) => {
        const g = [...document.querySelectorAll(`[role="${role}"]`)].find(
          (n) => n.getAttribute("aria-label") === label,
        );
        g.querySelector("[data-state]").click();
        return true;
      },
      "toolbar",
      "面板中显示的卡片",
    );
  }
  console.log(`tiles ${step}:`, JSON.stringify(await run(tileState, "toolbar", "面板中显示的卡片")));
}
console.log("back to overview:", await clickBy(clickTab, "概览"));

const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync("shots/chip-groups.png", Buffer.from(shot.result.data, "base64"));
console.log("wrote shots/chip-groups.png");
process.exit(0);
