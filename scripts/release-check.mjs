// Runs the release binary the way a user would get it — no dev server, no Vite — and asks the
// two questions that only a release build can answer: does the page still render with the CSP
// switched on, and what does the header actually say.
//
//   node scripts/release-check.mjs [port]
//
// It launches the exe with APPDATA pointed at a throwaway directory so the real settings and
// history are untouched, and stops only the process it started.
import { spawn } from "node:child_process";
import { mkdirSync, rmSync, existsSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";

const PORT = process.argv[2] ?? "9335";
const root = resolve(import.meta.dirname, "..");
const exe = resolve(root, "src-tauri/target/release/meerkat.exe");
const sandbox = resolve(root, "src-tauri/target/release/.sandbox-appdata");

if (!existsSync(exe)) {
  console.log("no release binary; run `pnpm tauri build` first");
  process.exit(1);
}
rmSync(sandbox, { recursive: true, force: true });
mkdirSync(sandbox, { recursive: true });

// A separate WebView2 user data folder, or the whole thing fails with 0x8007139F the moment a
// dev instance is open: WebView2 refuses a second environment on a folder another process has
// locked, and that reads as "the release build is broken" when it is only "the dev build is
// running". APPDATA and LOCALAPPDATA go the same way so the release instance cannot write the
// real settings or history either.
const child = spawn(exe, [], {
  detached: false,
  stdio: ["ignore", "pipe", "pipe"],
  env: {
    ...process.env,
    APPDATA: sandbox,
    LOCALAPPDATA: sandbox,
    WEBVIEW2_USER_DATA_FOLDER: resolve(sandbox, "ebview"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
  },
});
let stderr = "";
child.stderr.on("data", (chunk) => {
  stderr += chunk.toString();
});
let exited = null;
child.on("exit", (code) => {
  exited = code;
});

const fingerprint = (dir) =>
  existsSync(dir)
    ? readdirSync(dir).sort().map((f) => `${f}:${statSync(resolve(dir, f)).size}`).join(" ")
    : "(absent)";
const real0 = resolve(process.env.APPDATA ?? "", "com.tzz.meerkat");
const before = fingerprint(real0);

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const stop = () => {
  try {
    process.kill(child.pid);
  } catch {
    /* already gone */
  }
};
let targets = null;
for (let i = 0; i < 30 && !targets; i++) {
  await wait(1000);
  if (exited !== null) break;
  targets = await fetch(`http://127.0.0.1:${PORT}/json/list`).then((r) => r.json()).catch(() => null);
}
if (!targets) {
  console.log(`no DevTools endpoint on ${PORT}; child exit=${exited}`);
  if (stderr.trim()) console.log("stderr:", stderr.trim().split("\n").slice(0, 6).join("\n"));
  stop();
  rmSync(sandbox, { recursive: true, force: true });
  process.exit(1);
}

const page = targets.find((t) => t.type === "page" && !t.url.includes("panel"));
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r, e) => {
  socket.addEventListener("open", r, { once: true });
  socket.addEventListener("error", e, { once: true });
});
let id = 1;
const pend = new Map();
const seen = [];
socket.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pend.has(msg.id)) {
    pend.get(msg.id)(msg);
    pend.delete(msg.id);
  }
  if (msg.method === "Network.responseReceived") {
    const { response } = msg.params;
    seen.push({ url: response.url, status: response.status, csp: response.headers["content-security-policy"] ?? response.headers["Content-Security-Policy"] });
  }
});
const send = (method, params = {}) => {
  const i = id++;
  socket.send(JSON.stringify({ id: i, method, params }));
  return new Promise((r) => pend.set(i, r));
};
const run = async (fn, ...a) => {
  const r = await send("Runtime.evaluate", {
    expression: `(${fn.toString()})(${a.map((x) => JSON.stringify(x)).join(",")})`,
    returnByValue: true,
  });
  if (r.result?.exceptionDetails) return `EXC ${r.result.exceptionDetails.exception?.description}`;
  return r.result?.result?.value;
};

await send("Network.enable");
await send("Page.enable");
await send("Page.reload", { ignoreCache: true });
await wait(4000);

console.log("protocol:", page.url);
for (const r of seen.slice(0, 6)) console.log(`  ${r.status} ${r.url}  csp=${r.csp ?? "(absent)"}`);
console.log(
  "rendered:",
  JSON.stringify(
    await run(() => ({
      text: document.body.innerText.trim().length,
      tab: document.querySelector('[role="tab"][data-state="active"]')?.textContent?.trim(),
      tabs: document.querySelectorAll('[role="tab"]').length,
      shellDisplay: getComputedStyle(document.querySelector(".shell")).display,
      // An inline style attribute is the thing a CSP without 'unsafe-inline' in style-src kills
      // first: the row meters and the chart ceilings are all positioned that way.
      inlineStyleWorks: (() => {
        const el = document.createElement("div");
        el.setAttribute("style", "position:absolute;left:13px");
        document.body.appendChild(el);
        const ok = getComputedStyle(el).left === "13px";
        el.remove();
        return ok;
      })(),
      scripts: [...document.querySelectorAll("script[src]")].length,
    })),
  ),
);
// APPDATA cannot be redirected for a Windows app: the known-folder API answers from the token,
// not the environment. So instead of claiming isolation, measure whether this run touched the
// real data directory at all.
const real = resolve(process.env.APPDATA ?? "", "com.tzz.meerkat");
const now = fingerprint(real);
console.log(
  "real app data dir:",
  JSON.stringify(now) === JSON.stringify(before) ? "unchanged by this run ✓" : `CHANGED: ${before} -> ${now}`,
);

stop();
await wait(500);
rmSync(sandbox, { recursive: true, force: true });
console.log("stopped the release instance and removed the sandbox");
process.exit(0);
