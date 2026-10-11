// Drives the probe browser window over the DevTools protocol so the app list can be
// measured in a real viewport: how many rows are mounted, whether scrolling reaches the
// last app, and whether a row that has been scrolled into view still responds to the
// context menu. Nothing here closes an application.
const ENDPOINT = "http://127.0.0.1:9333";

const list = await fetch(`${ENDPOINT}/json/list`).then((r) => r.json());
const page = list.find((t) => t.type === "page" && t.url.includes("localhost:1420"));
if (!page) {
  console.log(JSON.stringify({ error: "no probe page", urls: list.map((t) => t.url) }));
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

function send(method, params) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

// The page can be left on any tab by earlier runs, and the tab decides which list is on
// screen, so the probe navigates explicitly instead of trusting the window's history.
await send("Page.enable", {});
await send("Page.navigate", { url: process.argv[2] ?? "http://localhost:1420/?tab=cpu" });
await new Promise((resolve) => setTimeout(resolve, 1200));

async function evaluate(functionBody) {
  const response = await send("Runtime.evaluate", {
    expression: `(${functionBody})()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (response.exceptionDetails) {
    return { error: response.exceptionDetails.exception?.description ?? response.exceptionDetails.text };
  }
  if (response.error) return { error: `devtools: ${response.error.message}` };
  const value = response.result?.result?.value;
  if (value === undefined) return { error: "no value", response };
  return value;
}

const result = await evaluate(`async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const content = document.querySelector(".content");
  // Two lists share the row markup: the insight strip and the application table.
  const rowsEl = document.querySelector(".app-table .rows");

  const audit = () => {
    const rows = [...rowsEl.querySelectorAll(":scope > .row")];
    const tops = rows
      .map((r) => Math.round(r.getBoundingClientRect().top))
      .sort((a, b) => a - b);
    const gaps = [];
    for (let i = 1; i < tops.length; i++) gaps.push(tops[i] - tops[i - 1]);
    const rects = rows.map((r) => r.getBoundingClientRect());
    return {
      mounted: rows.length,
      count: Number(rowsEl ? rowsEl.dataset.count : 0),
      nodes: document.getElementsByTagName("*").length,
      htmlKB: Math.round((rowsEl ? rowsEl.innerHTML.length : 0) / 1024),
      scrollTop: Math.round(content.scrollTop),
      viewport: Math.round(content.clientHeight),
      scrollHeight: content.scrollHeight,
      listHeight: rowsEl ? Math.round(rowsEl.getBoundingClientRect().height) : 0,
      rowLists: document.querySelectorAll(".rows").length,
      gapSizes: [...new Set(gaps)].sort((a, b) => a - b),
      heights: [...new Set(rows.map((r) => r.offsetHeight))].sort((a, b) => a - b),
      tall: rows
        .filter((r) => r.offsetHeight > 40)
        .map((r) => (r.querySelector(".row-name")?.textContent ?? "") + "|" + r.offsetHeight),
      emptyNames: rows.filter((r) => !(r.querySelector(".row-name")?.textContent ?? "").trim()).length,
      aboveViewport: tops.filter((t) => t < content.getBoundingClientRect().top - 40).length,
      belowViewport: tops.filter(
        (t) => t > content.getBoundingClientRect().bottom + 40,
      ).length,
      first: rows[0] ? rows[0].querySelector(".row-name").textContent : null,
      last: rows.length ? rows[rows.length - 1].querySelector(".row-name").textContent : null,
      indexes: rows.map((r) => Number(r.dataset.index)),
    };
  };

  // Wait for the snapshot to arrive before measuring anything.
  for (let i = 0; i < 30; i++) {
    if (document.querySelectorAll(".rows > .row").length > 0) break;
    await sleep(200);
  }

  content.scrollTop = 0;
  await sleep(400);
  const top = audit();

  content.scrollTop = Math.round(content.scrollHeight / 2);
  await sleep(400);
  const middle = audit();

  content.scrollTop = content.scrollHeight;
  await sleep(400);
  const bottom = audit();

  // Right click has to reach a row that exists only because it was scrolled to. The pin is
  // checked from the top of the list, because pinning sorts the row to the front — testing it
  // from the middle would unmount the very row the test is looking for.
  content.scrollTop = 0;
  await sleep(450);
  // Right click opens the row menu; its first item is Pin / Unpin. Pins persist in
  // localStorage now, so whatever this pins it also unpins before returning.
  const clicked = async (el) => {
    el.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40, button: 2 }),
    );
    await sleep(80);
    const item = document.querySelector('[role="menu"] [role="menuitem"]');
    if (item) item.click();
    return Boolean(item);
  };
  // Anything a previous run left pinned would confuse the ordering check below.
  for (const row of [...document.querySelectorAll(".row.pinned")]) await clicked(row);
  await sleep(400);

  const rows2 = [...rowsEl.querySelectorAll(":scope > .row")];
  const target = rows2[Math.min(3, rows2.length - 1)];
  const name = target ? target.querySelector(".row-name").textContent : null;
  let interaction = { skipped: "no rows mounted" };
  if (target) {
    const menuOpened = await clicked(target);
    await sleep(450);
    const after = [...rowsEl.querySelectorAll(":scope > .row")];
    interaction = {
      target: name,
      menuOpened,
      pinnedNames: after.filter((r) => r.classList.contains("pinned")).map((r) => r.querySelector(".row-name").textContent),
      movedToFront: after[0] ? after[0].querySelector(".row-name").textContent === name : false,
      hasQuitButton: Boolean(target.querySelector(".row-quit")),
      hasMeter: Boolean(target.querySelector(".row-meter")),
      tooltip: Boolean(target.getAttribute("title")),
    };
    for (const row of [...document.querySelectorAll(".row.pinned")]) await clicked(row);
    await sleep(400);
    interaction.pinnedCleared = document.querySelectorAll(".row.pinned").length === 0;
  }

  return {
    totalRowsInData: Math.round((rowsEl ? rowsEl.getBoundingClientRect().height : 0) / 37),
    top,
    middle,
    bottom,
    interaction,
    pinnedAtEnd: document.querySelectorAll(".row.pinned").length,
    viewportSize: window.innerWidth + "x" + window.innerHeight,
  };
}`);

console.log(JSON.stringify(result, null, 2));
socket.close();
