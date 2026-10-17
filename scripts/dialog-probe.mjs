// Exercises the quit dialog end to end: it opens, focus lands on the safe action, cancelling
// closes it, and — the thing Radix's scroll lock could plausibly break in a window whose
// scroller is not the body — the page is still interactive afterwards.
// Nothing here confirms a quit; the last click is always Cancel.
const targets = await fetch(`http://127.0.0.1:${process.env.MEERKAT_CDP ?? 9334}/json/list`).then((r) => r.json());
const page = targets.find((t) => t.type === "page" && !t.url.includes("panel"));
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => socket.addEventListener("open", r));
let id = 0;
const pending = new Map();
socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  }
});
const send = (method, params) => {
  const n = ++id;
  socket.send(JSON.stringify({ id: n, method, params }));
  return new Promise((r) => pending.set(n, r));
};
const ev = async (expression) =>
  (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;

const body = `(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const out = {};
  const quit = document.querySelector(".row .row-quit");
  out.openedFrom = quit ? (quit.closest(".row").querySelector(".row-name")?.textContent ?? "?") : null;
  quit.click();
  await sleep(350);
  const content = document.querySelector('[role="dialog"]');
  out.dialog = content ? content.textContent.slice(0, 46) : null;
  const cs = content && getComputedStyle(content);
  out.box = cs && (cs.padding + " | " + cs.borderRadius + " | " + cs.backgroundColor + " | z" + cs.zIndex);
  out.focus = document.activeElement ? (document.activeElement.textContent || document.activeElement.tagName) : null;
  out.bodyPointerEvents = getComputedStyle(document.body).pointerEvents;
  // Escape is Radix's own binding now, so test the button instead: it is the one path that
  // must actually work while the backdrop is up.
  const cancel = [...document.querySelectorAll('[role="dialog"] button')].find((b) => b.textContent.trim().length < 8);
  cancel.click();
  await sleep(450);
  out.afterCancel = {
    dialog: Boolean(document.querySelector('[role="dialog"]')),
    bodyPointerEvents: getComputedStyle(document.body).pointerEvents,
  };
  const scroller = document.querySelector(".content");
  scroller.scrollTop = 320;
  await sleep(120);
  out.scrollWorks = Math.round(scroller.scrollTop);
  const tab = [...document.querySelectorAll(".tab")].find((t) => t.textContent.trim() === "内存");
  tab.click();
  await sleep(400);
  out.tabWorks = document.querySelector('.tab[aria-selected="true"]')?.textContent.trim();
  out.gameAlive = true;
  return JSON.stringify(out, null, 1);
})()`;

console.log(await ev(body));
socket.close();
