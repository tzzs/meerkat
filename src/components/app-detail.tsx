import { useEffect, useRef, useState } from "react";
import { Copy, FolderOpen, X } from "lucide-react";
import { cpuDisplay, formatBytes, formatDuration, formatPercent, formatRate, formatTime } from "../lib/format";
import { ipc } from "../lib/ipc";
import { useStore, type AppSeed } from "../lib/store";
import { appLabel } from "../lib/apps";
import { useLoaded } from "../lib/use-loaded";
import type { AppInfo, AppPoint, Priority, ProcessRow, WslMemory } from "../lib/types";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";
import { AppIcon, Chart, Stat } from "./ui";
import { Button } from "./ui/button";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
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

/** An app that is not running, as far as the detail sheet needs: a name and a file, no figures. */
function ghostOf(seed: AppSeed): AppInfo {
  return {
    ...seed,
    mainPid: 0,
    processCount: 0,
    cpuPercent: 0,
    memoryBytes: 0,
    readBytesPerSecond: 0,
    writeBytesPerSecond: 0,
    readBytesTotal: 0,
    writeBytesTotal: 0,
    connections: 0,
    listeningPorts: [],
    oldestStartedAtMs: null,
    windows: [],
    pids: [],
    unmeasuredProcesses: 0,
    services: [],
    canQuit: false,
    control: { efficiency: false, priority: null, muteAlerts: false, paused: false },
    project: null,
  };
}

const HISTORY_RANGES = [
  { seconds: 86_400, key: "range.24h" },
  { seconds: 7 * 86_400, key: "range.7d" },
  { seconds: 30 * 86_400, key: "range.30d" },
] as const;

/**
 * The app's own line through Pulse's history: CPU and memory over a day, a week or a month, and
 * what that adds up to. This is the part a list cannot show, and the part that answers "has it
 * always used this much, or did it start growing yesterday".
 */
function AppHistory({ appKey }: { appKey: string }) {
  const { t, units } = useStore();
  const [range, setRange] = useState<number>(86_400);
  const points = useLoaded(() => ipc.appSeries(appKey, range), [] as AppPoint[], [appKey, range]);
  const cpuSeconds = points.reduce((sum, p) => sum + (p.cpuPercent / 100) * p.spanSeconds, 0);
  const written = points.reduce((sum, p) => sum + p.writeBytes, 0);
  const memory = points.length ? points.reduce((sum, p) => sum + p.memoryBytes, 0) / points.length : 0;
  return (
    <section>
      <div className="flex items-center justify-between gap-2">
        <h4 className="section-title">{t("history.appHistory")}</h4>
        <ToggleGroup type="single" value={String(range)} onValueChange={(next) => next && setRange(Number(next))}>
          {HISTORY_RANGES.map((r) => (
            <ToggleChip key={r.seconds} value={String(r.seconds)}>
              {t(r.key)}
            </ToggleChip>
          ))}
        </ToggleGroup>
      </div>
      {points.length < 2 ? (
        <p className="empty faint">{t("history.appHistoryEmpty")}</p>
      ) : (
        <>
          <div className="stat-label mt-2">{t("metric.cpu")}</div>
          <Chart values={points.map((p) => cpuDisplay(p.cpuPercent, units))} color="var(--m-cpu)" height={44} />
          <div className="stat-label mt-2">{t("metric.memory")}</div>
          <Chart values={points.map((p) => p.memoryBytes)} color="var(--m-memory)" height={44} />
          <p className="hint faint">
            {t("history.cpuTime", { value: formatDuration(cpuSeconds, units.locale, { short: true }) })} ·{" "}
            {formatBytes(memory, units)} · {t("history.written", { value: formatBytes(written, units) })}
          </p>
        </>
      )}
    </section>
  );
}

const PRIORITIES: Priority[] = ["low", "belowNormal", "normal", "aboveNormal", "high"];

/**
 * vmmemWSL is one row of several gigabytes. Linux inside it knows how much of that is programs
 * and how much is file cache it would give back; asked once when this sheet opens.
 */
function WslBreakdown() {
  const { t, units } = useStore();
  const [memory, setMemory] = useState<WslMemory | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    void ipc.wslMemory().then((m) => alive && setMemory(m));
    return () => {
      alive = false;
    };
  }, []);
  if (memory === undefined) return <p className="empty faint">…</p>;
  if (memory === null) return null;
  const parts: [string, number, string][] = [
    [t("wsl.used"), memory.usedBytes, "var(--m-memory)"],
    [t("wsl.cache"), memory.cacheBytes, "color-mix(in srgb, var(--m-memory) 45%, transparent)"],
    [t("wsl.free"), memory.freeBytes, "var(--surface-3)"],
  ];
  return (
    <section>
      <h4 className="section-title">{t("wsl.title")}</h4>
      <div className="flex h-2.5 overflow-hidden rounded-full">
        {parts.map(([label, bytes, color]) => (
          <span key={label} style={{ flexGrow: bytes, background: color }} />
        ))}
      </div>
      <div className="legend mt-1.5">
        {parts.map(([label, bytes]) => (
          <span key={label}>
            {label} {formatBytes(bytes, units)}
          </span>
        ))}
      </div>
      <p className="hint faint">{t("wsl.help", { distros: memory.distros.join(", ") })}</p>
    </section>
  );
}

/**
 * How the app runs, remembered: efficiency mode, a priority, alerts muted, all applied again to
 * every process the app starts. And pausing it, which is not remembered, because a paused app
 * nobody remembers pausing is a bug report waiting to happen.
 */
function AppControls({ app, onNotice }: { app: AppInfo; onNotice: (text: string) => void }) {
  const { t } = useStore();
  const [busy, setBusy] = useState(false);
  const control = app.control;
  const save = async (change: Partial<{ efficiency: boolean; priority: Priority | null; muteAlerts: boolean }>) => {
    setBusy(true);
    const error = await ipc.setRule({
      key: app.key,
      name: app.name,
      efficiency: control.efficiency,
      priority: control.priority,
      muteAlerts: control.muteAlerts,
      ...change,
    });
    setBusy(false);
    onNotice(error ? t("control.failed", { reason: error }) : t("control.saved"));
  };
  const pause = async (next: boolean) => {
    setBusy(true);
    const error = await ipc.pauseApp(app.key, next);
    setBusy(false);
    onNotice(error ? t("control.failed", { reason: error }) : next ? t("control.pausedNotice") : t("control.resumed"));
  };
  return (
    <section>
      <h4 className="section-title">{t("control.title")}</h4>
      <div className="space-y-2.5 text-[12.5px]">
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            className="mt-0.5"
            checked={control.efficiency}
            disabled={busy || !app.canQuit}
            onChange={(e) => void save({ efficiency: e.target.checked })}
          />
          <span>
            {t("control.efficiency")}
            <span className="block text-[11px] text-ink-3">{t("control.efficiencyHelp")}</span>
          </span>
        </label>
        <div className="flex items-center gap-2">
          <span className="flex-1">{t("control.priority")}</span>
          <Select
            value={control.priority ?? "auto"}
            disabled={busy || !app.canQuit || control.efficiency}
            onValueChange={(next) => void save({ priority: next === "auto" ? null : (next as Priority) })}
          >
            <SelectTrigger aria-label={t("control.priority")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">{t("control.priorityAuto")}</SelectItem>
              {PRIORITIES.map((p) => (
                <SelectItem key={p} value={p}>
                  {t(`control.priority.${p}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            className="mt-0.5"
            checked={control.muteAlerts}
            disabled={busy}
            onChange={(e) => void save({ muteAlerts: e.target.checked })}
          />
          <span>
            {t("control.mute")}
            <span className="block text-[11px] text-ink-3">{t("control.muteHelp")}</span>
          </span>
        </label>
        {app.canQuit && (
          <div className="flex items-center gap-2">
            <span className="flex-1 text-[11px] text-ink-3">{control.paused ? t("control.pausedHelp") : t("control.pauseHelp")}</span>
            <Button variant={control.paused ? "primary" : "chip"} disabled={busy} onClick={() => void pause(!control.paused)}>
              {control.paused ? t("control.resume") : t("control.pause")}
            </Button>
          </div>
        )}
      </div>
    </section>
  );
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
  const { snapshot, detailKey, detailSeed, closeDetail, t, units, requestQuit } = useStore();
  const live = snapshot?.apps.find((a) => a.key === detailKey) ?? null;
  // The last live reading of this app, kept on screen if it exits while the sheet is open.
  const lastLive = useRef<AppInfo | null>(null);
  if (live) lastLive.current = live;
  const remembered = lastLive.current?.key === detailKey ? lastLive.current : null;
  // Opened from the history for an app that is not running: only its name is known.
  const ghost = !live && !remembered && detailSeed?.key === detailKey && detailSeed ? ghostOf(detailSeed) : null;
  const current = live ?? remembered ?? ghost;
  // Whatever was on screen stays there through the sheet's exit animation, after the store has
  // already forgotten which app it was.
  const shown = useRef<AppInfo | null>(null);
  if (current) shown.current = current;
  const app = detailKey ? current : shown.current;
  const processes = useProcesses(live ? detailKey : null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => setNotice(null), [detailKey]);

  if (!app) return null;
  const running = Boolean(live);
  const exited = Boolean(detailKey) && !live && Boolean(remembered);
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
              {t(`kind.${app.kind}`)}
              {running || exited ? ` · ${app.processCount} ${t("label.processes")}` : ""}
              {exited ? ` · ${t("detail.exited")}` : running ? "" : ` · ${t("history.notRunning")}`}
              {app.control.paused ? ` · ${t("control.paused")}` : ""}
            </div>
          </div>
          <SheetClose asChild>
            <Button variant="tool" aria-label={t("action.close")}>
              <X aria-hidden className="size-4" />
            </Button>
          </SheetClose>
        </header>

        <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
          {(running || exited) && (
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
          )}

          {running && app.key === "image:vmmemwsl" && <WslBreakdown />}

          {running && <AppControls app={app} onNotice={say} />}

          <AppHistory appKey={app.key} />

          {app.project && (
            <section>
              <h4 className="section-title">{t("detail.project")}</h4>
              <div className="flex items-center gap-1">
                <code className="min-w-0 flex-1 truncate font-num text-[11.5px] text-ink-2">{app.project}</code>
                <Hint label={t("detail.openFolder")}>
                  <Button variant="tool" aria-label={t("detail.openFolder")} onClick={() => ipc.openProject(app.key)}>
                    <FolderOpen aria-hidden className="size-3.5" />
                  </Button>
                </Hint>
              </div>
            </section>
          )}

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

          {running && (
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
          )}
        </div>

        <footer className="flex min-h-[52px] items-center gap-2 border-t border-input px-5 py-2.5">
          <span className="min-w-0 flex-1 truncate text-[11.5px] text-ink-3" role="status">
            {notice ?? (app.canQuit || !running ? "" : t("list.cannotQuit"))}
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
