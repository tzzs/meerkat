import { useEffect, useRef, useState } from "react";
import { Copy, FolderOpen, X } from "lucide-react";
import { cpuDisplay, formatBytes, formatDuration, formatPercent, formatRate, formatTime } from "../lib/format";
import { ipc } from "../lib/ipc";
import { useStore } from "../lib/store";
import { appLabel } from "../lib/apps";
import type { AppInfo, ProcessRow } from "../lib/types";
import { AppIcon, Stat } from "./ui";
import { Button } from "./ui/button";
import { Sheet, SheetClose, SheetContent } from "./ui/sheet";
import { Hint } from "./ui/tooltip";

/** The processes behind the open app, refreshed while the sheet is up and not otherwise. */
function useProcesses(key: string | null): ProcessRow[] {
  const [rows, setRows] = useState<ProcessRow[]>([]);
  useEffect(() => {
    setRows([]);
    if (!key) return;
    let alive = true;
    const load = () => void ipc.appProcesses(key).then((next) => alive && next.length > 0 && setRows(next));
    load();
    const timer = window.setInterval(load, 1500);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [key]);
  return rows;
}

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Everything Pulse knows about one app, opened by clicking its row. The lists show one line per
 * app because that is what a list is for; this is where the line opens up: which processes it
 * is made of and what each was started with, its windows, its ports, the services inside it,
 * how long it has been running and how much it has read and written since.
 */
export function AppDetailSheet() {
  const { snapshot, detailKey, closeDetail, t, units, requestQuit } = useStore();
  const live = snapshot?.apps.find((a) => a.key === detailKey) ?? null;
  // Keeps the last reading on screen when the app exits under the open sheet, and through the
  // sheet's own exit animation after the store has already forgotten which app it was.
  const last = useRef<AppInfo | null>(null);
  if (live) last.current = live;
  const app = live ?? last.current;
  const processes = useProcesses(detailKey);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => setNotice(null), [detailKey]);

  if (!app) return null;
  const exited = Boolean(detailKey) && !live;
  const label = appLabel(app, t);
  const now = snapshot?.takenAtMs ?? Date.now();
  const runningFor = app.oldestStartedAtMs ? (now - app.oldestStartedAtMs) / 1000 : null;
  // Hidden top-level windows are the plumbing every program has (IME windows, message windows,
  // one per Chromium tab); only the ones on screen are something to switch to.
  const windows = app.windows.filter(
    (w, index, all) => w.visible && all.findIndex((other) => other.visible && other.title === w.title) === index,
  );
  const say = (text: string) => setNotice(text);

  return (
    <Sheet open={Boolean(detailKey)} onOpenChange={(open) => !open && closeDetail()}>
      <SheetContent label={t("detail.title", { name: label })}>
        <header className="flex items-center gap-3 border-b border-input px-5 py-4">
          <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
          <div className="min-w-0 flex-1">
            <div className="truncate font-display text-[15px] font-[650] tracking-[-0.01em]">{label}</div>
            <div className="truncate text-[11.5px] text-ink-3">
              {t(`kind.${app.kind}`)} · {app.processCount} {t("label.processes")}
              {exited ? ` · ${t("detail.exited")}` : ""}
            </div>
          </div>
          <SheetClose asChild>
            <Button variant="tool" aria-label={t("action.close")}>
              <X aria-hidden className="size-4" />
            </Button>
          </SheetClose>
        </header>

        <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
          <section className="detail-figures grid grid-cols-2 gap-x-6">
            <Stat
              label={t("metric.cpu")}
              value={formatPercent(cpuDisplay(app.cpuPercent, units), units.locale, app.cpuPercent >= 10 ? 0 : 1)}
            />
            <Stat label={t("metric.memory")} value={formatBytes(app.memoryBytes, units)} />
            <Stat
              label={t("label.readWrite")}
              value={`${formatRate(app.readBytesPerSecond, units, "storage")} / ${formatRate(app.writeBytesPerSecond, units, "storage")}`}
            />
            <Stat label={t("label.connections")} value={String(app.connections)} />
            <Stat
              label={t("detail.runningFor")}
              value={runningFor === null ? "—" : formatDuration(runningFor, units.locale, { short: true })}
              sub={app.oldestStartedAtMs ? t("detail.since", { time: formatTime(app.oldestStartedAtMs, units.locale) }) : undefined}
            />
            <Stat
              label={t("detail.ioSinceStart")}
              value={`${formatBytes(app.readBytesTotal, units)} / ${formatBytes(app.writeBytesTotal, units)}`}
            />
          </section>

          {app.exePath && (
            <section>
              <h4 className="section-title">{t("detail.location")}</h4>
              <div className="flex items-center gap-1">
                <Hint className="max-w-[440px] break-all font-num text-[11px]" label={app.exePath}>
                  <code className="min-w-0 flex-1 truncate font-num text-[11.5px] text-ink-2">{app.exePath}</code>
                </Hint>
                <Hint label={t("action.copyProcessPath")}>
                  <Button
                    variant="tool"
                    aria-label={t("action.copyProcessPath")}
                    onClick={() => void copy(app.exePath).then((ok) => say(ok ? t("detail.copied") : t("detail.copyFailed")))}
                  >
                    <Copy aria-hidden className="size-3.5" />
                  </Button>
                </Hint>
                <Hint label={t("detail.reveal")}>
                  <Button variant="tool" aria-label={t("detail.reveal")} onClick={() => ipc.revealInExplorer(app.exePath)}>
                    <FolderOpen aria-hidden className="size-3.5" />
                  </Button>
                </Hint>
              </div>
            </section>
          )}

          {app.services.length > 0 && (
            <section>
              <h4 className="section-title">{t("detail.services")}</h4>
              <ul className="m-0 list-none space-y-1 p-0 text-[12.5px]">
                {app.services.map((service) => (
                  <li key={service}>{service}</li>
                ))}
              </ul>
            </section>
          )}

          {windows.length > 0 && (
            <section>
              <h4 className="section-title">{t("list.windows")}</h4>
              <ul className="m-0 list-none space-y-1 p-0">
                {windows.map((window) => (
                  <li key={window.hwnd} className="flex items-center gap-2 text-[12.5px]">
                    <span className="min-w-0 flex-1 truncate">{window.title}</span>
                    {!exited && (
                      <Button
                        variant="chip"
                        onClick={() =>
                          void ipc.focusWindow(window.hwnd).then((error) => error && say(t("detail.focusFailed")))
                        }
                      >
                        {t("detail.switchTo")}
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          )}

          {app.listeningPorts.length > 0 && (
            <section>
              <h4 className="section-title">{t("list.openPorts")}</h4>
              <p className="m-0 font-num text-[12px] text-ink-2">{app.listeningPorts.join("  ")}</p>
            </section>
          )}

          <section>
            <h4 className="section-title">
              {t("label.processes")} · {processes.length || app.processCount}
            </h4>
            <div className="divide-y divide-[var(--hairline)]">
              {processes.map((row) => {
                const command = row.commandLine.join(" ");
                return (
                  <div key={row.pid} className="py-1.5">
                    <div className="flex items-baseline gap-2 text-[12.5px]">
                      <span className="min-w-0 flex-1 truncate">
                        {row.name} <span className="font-num text-[11px] text-ink-3">{row.pid}</span>
                      </span>
                      <span className="w-14 text-right font-num text-[11.5px] text-ink-2">
                        {formatPercent(cpuDisplay(row.cpuPercent, units), units.locale, row.cpuPercent >= 10 ? 0 : 1)}
                      </span>
                      <span className="w-[72px] text-right font-num text-[11.5px]">{formatBytes(row.memoryBytes, units)}</span>
                    </div>
                    {command ? (
                      <Hint className="max-w-[440px] break-all font-num text-[11px]" label={command}>
                        <div className="truncate font-num text-[10.5px] text-ink-3">{command}</div>
                      </Hint>
                    ) : (
                      <div className="text-[10.5px] text-ink-3">
                        {row.canQuit ? t("detail.noCommandLine") : t("detail.commandLineHidden")}
                      </div>
                    )}
                  </div>
                );
              })}
              {processes.length === 0 && <p className="empty faint">…</p>}
            </div>
          </section>
        </div>

        <footer className="flex min-h-[52px] items-center gap-2 border-t border-input px-5 py-2.5">
          <span className="min-w-0 flex-1 truncate text-[11.5px] text-ink-3" role="status">
            {notice ?? (app.canQuit ? "" : t("list.cannotQuit"))}
          </span>
          {app.canQuit && !exited && (
            <Button variant="danger" onClick={() => requestQuit(app)}>
              {t("action.quitApp")}
            </Button>
          )}
        </footer>
      </SheetContent>
    </Sheet>
  );
}
