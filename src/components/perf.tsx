import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent } from "react";
import { Check, Trash2, X } from "lucide-react";
import { formatBytes, formatDuration, formatTime } from "../lib/format";
import { ipc, listenEvent } from "../lib/ipc";
import { subscribe } from "../lib/subscribe";
import {
  PERF_METRICS,
  formatPerfStat,
  formatPerfValue,
  perfAppChoices,
  pickerSignature,
  perfHeadlineKey,
  perfIssueKey,
  perfIssueValue,
  perfSampleSeries,
  perfSeries,
  perfTargetLabel,
  perfVerdictKey,
  type PerfMetric,
} from "../lib/perf";
import { appLabel } from "../lib/apps";
import { useStore } from "../lib/store";
import type { AppInfo, PerfReport, PerfSample, PerfSession, PerfSessionSummary, PerfStatus, PerfTargetMode, PerfVerdict } from "../lib/types";
import type { Translate } from "../i18n";
import type { UnitPrefs } from "../lib/format";
import { Button } from "./ui/button";
import { AppIcon, Card, Chart, Pill, Sections, Stat } from "./ui";
import { Sheet, SheetClose, SheetContent } from "./ui/sheet";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import { Hint } from "./ui/tooltip";

const VERDICT_TONE: Record<PerfVerdict, string> = {
  healthy: "accent",
  warning: "warn",
  problem: "danger",
  inconclusive: "note",
};

const SESSION_LIMIT = 30;

const SECTIONS = ["setup", "live", "history"];
const MAX_LIVE_SAMPLES = 120;

const EMPTY_STATUS: PerfStatus = {
  active: false,
  sessionId: null,
  targetName: "",
  mode: "foreground",
  currentAppKey: null,
  sampleCount: 0,
  startedAtMs: null,
  latest: null,
};

function unavailableReason(metric: PerfMetric, t: Translate): string {
  if (metric.key === "fps") return t("perf.fpsUnavailable");
  if (metric.key === "cpuTempC") return t("perf.cpuTempUnavailable");
  return t("state.unavailable");
}

/** The length of a finished session; a session still recording has no answer. */
function sessionSeconds(session: Pick<PerfSessionSummary, "startedAtMs" | "endedAtMs">): number | null {
  return session.endedAtMs === null ? null : (session.endedAtMs - session.startedAtMs) / 1000;
}

/**
 * One row of the app picker.
 *
 * The picker lists every running app at once, and the tab re-renders on each snapshot — once
 * a second — because it reads the store. Rebuilding ninety-odd rows for figures they do not
 * show measured as a dropped frame on every tick; with a memo of its own, each row is skipped
 * unless its own app changed. Every prop is a primitive and the handler reads the key off the
 * button, so the memo actually holds.
 */
const PickerRow = memo(function PickerRow({
  id,
  name,
  exePath,
  processCount,
  processLabel,
  selected,
  disabled,
  onSelect,
}: {
  id: string;
  name: string;
  exePath: string;
  processCount: number;
  processLabel: string;
  selected: boolean;
  disabled: boolean;
  onSelect: (event: MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      type="button"
      className="perf-app"
      data-key={id}
      aria-pressed={selected}
      disabled={disabled}
      onClick={onSelect}
    >
      <AppIcon name={name} seed={id} exePath={exePath || undefined} />
      <span className="perf-app-main">
        <span className="perf-app-name">{name}</span>
        <span className="perf-app-meta">
          {processCount} {processLabel}
        </span>
      </span>
      {selected && <Check aria-hidden className="perf-app-check size-3.5" />}
    </button>
  );
});

/**
 * The picker's rows, as one memoised block.
 *
 * The tab re-renders on every snapshot; this list only changes when the set of apps does. The
 * memo keeps React from even building the rows' elements on the ticks that change nothing
 * they show.
 */
const AppPicker = memo(function AppPicker({
  apps,
  selectedKey,
  disabled,
  processLabel,
  t,
  ariaLabel,
  onSelect,
}: {
  apps: readonly AppInfo[];
  selectedKey: string | null;
  disabled: boolean;
  processLabel: string;
  t: Translate;
  ariaLabel: string;
  onSelect: (event: MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <div className="perf-apps" role="group" aria-label={ariaLabel}>
      {apps.map((app) => (
        <PickerRow
          key={app.key}
          id={app.key}
          name={appLabel(app, t)}
          exePath={app.exePath}
          processCount={app.processCount}
          processLabel={processLabel}
          selected={app.key === selectedKey}
          disabled={disabled}
          onSelect={onSelect}
        />
      ))}
    </div>
  );
});

function LiveTile({
  metric,
  value,
  vramBytes,
  series,
  units,
  t,
}: {
  metric: PerfMetric;
  value: number | null;
  vramBytes: number | null;
  series: number[];
  units: UnitPrefs;
  t: Translate;
}) {
  const note = unavailableReason(metric, t);
  const style = { ["--tone" as string]: metric.tone } as CSSProperties;
  return (
    <div className="metric-card" style={style}>
      <div className="metric-top">
        <span className="metric-label">{t(metric.labelKey)}</span>
        <span className="metric-value num">{formatPerfValue(value, metric.unit, units)}</span>
      </div>
      {value === null ? (
        <Hint label={note}>
          <span className="metric-sub">{note}</span>
        </Hint>
      ) : vramBytes !== null ? (
        <span className="metric-sub">{t("perf.vram.used", { bytes: formatBytes(vramBytes, units) })}</span>
      ) : null}
      <Chart values={series} ceiling={metric.ceiling} color={metric.tone} variant="line" height={30} />
    </div>
  );
}

function MetricReportCard({
  metric,
  session,
  units,
  t,
}: {
  metric: PerfMetric;
  session: PerfSession;
  units: UnitPrefs;
  t: Translate;
}) {
  const stat = session.report?.stats.find((entry) => entry.key === metric.key);
  const shown = stat?.available ? stat : undefined;
  const figures: [string, number | null][] = [
    [t("perf.stat.min"), shown?.min ?? null],
    [t("perf.stat.avg"), shown?.avg ?? null],
    [t("perf.stat.max"), shown?.max ?? null],
    [t("perf.stat.p95"), shown?.p95 ?? null],
    [t("perf.stat.last"), shown?.last ?? null],
  ];
  const vramBytes = metric.key === "vramPercent" ? session.report?.stats.find((entry) => entry.key === "vramUsedBytes") : undefined;
  return (
    <Card title={t(metric.labelKey)} tone={metric.tone}>
      <div className="detail-figures grid grid-cols-5 gap-2">
        {figures.map(([label, value]) => (
          <Stat key={label} label={label} value={formatPerfValue(value, metric.unit, units)} />
        ))}
      </div>
      {vramBytes && (
        <p className="hint faint">{t("perf.vram.used", { bytes: formatPerfStat(vramBytes, "bytes", units) })}</p>
      )}
      <Chart values={perfSeries(session, metric.key)} ceiling={metric.ceiling} color={metric.tone} height={72} />
    </Card>
  );
}

/**
 * One session's result, as a destination rather than an expansion under the table: the row
 * that opened it stays where it was, and opening the same row again brings the same sheet back.
 */
function ResultSheet({
  summary,
  report,
  loading,
  onClose,
  onDelete,
}: {
  summary: PerfSessionSummary | null;
  report: PerfSession | null;
  loading: boolean;
  onClose: () => void;
  onDelete: (id: string) => void;
}) {
  const { units, t } = useStore();
  // Radix keeps the content mounted for the exit animation, after the id it came from is gone.
  const last = useRef<{ summary: PerfSessionSummary; report: PerfSession | null } | null>(null);
  if (summary) last.current = { summary, report };
  const shown = summary ? { summary, report } : last.current;
  if (!shown) return null;

  const modeLabel = t(shown.summary.mode === "app" ? "perf.mode.app" : "perf.mode.foreground");
  const name = perfTargetLabel({ mode: shown.summary.mode, name: shown.summary.targetName }, modeLabel);
  const seconds = sessionSeconds(shown.summary);
  const session = shown.report?.id === shown.summary.id ? shown.report : null;
  const full: PerfReport | null = session?.report ?? null;
  const appCpu = full ? formatPerfStat(full.stats.find((entry) => entry.key === "appCpuPercent"), "percent", units) : "—";
  const appGpu = full ? formatPerfStat(full.stats.find((entry) => entry.key === "appGpuPercent"), "percent", units) : "—";

  return (
    <Sheet open={summary !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent label={t("perf.report.title")} className="w-[min(560px,94vw)]">
        <header className="flex items-center gap-3 border-b border-input px-5 py-4">
          <div className="min-w-0 flex-1">
            <div className="truncate font-display text-[15px] font-[650] tracking-[-0.01em]">{name}</div>
            <div className="truncate text-[11.5px] text-ink-3">
              {t("perf.report.meta", {
                time: formatTime(shown.summary.startedAtMs, units.locale),
                mode: modeLabel,
                duration: seconds === null ? "—" : formatDuration(seconds, units.locale, { short: true }),
              })}
            </div>
          </div>
          <SheetClose asChild>
            <Button variant="tool" aria-label={t("action.close")}>
              <X aria-hidden className="size-4" />
            </Button>
          </SheetClose>
        </header>

        <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
          {full && session ? (
            <>
              <section className="space-y-2">
                <Pill tone={VERDICT_TONE[full.verdict]}>{t(perfVerdictKey(full.verdict))}</Pill>
                <p className="m-0 text-[13.5px]">{t(perfHeadlineKey(full))}</p>
                <p className="m-0 text-[11.5px] text-ink-3">
                  {t("perf.report.samples", {
                    count: full.sampleCount,
                    duration: formatDuration(full.durationSeconds, units.locale),
                  })}
                </p>
                <p className="m-0 text-[11.5px] text-ink-3">{t("perf.report.appDetail", { cpu: appCpu, gpu: appGpu })}</p>
              </section>

              <section>
                <h4 className="section-title">{t("perf.report.findings")}</h4>
                {full.issues.length === 0 ? (
                  <p className="empty faint">{t("perf.report.noFindings")}</p>
                ) : (
                  <ul className="m-0 list-none space-y-2 p-0">
                    {full.issues.map((issue) => (
                      <li key={`${issue.kind}:${issue.metric}`} className="flex items-baseline gap-2">
                        <Pill tone={VERDICT_TONE[issue.verdict]}>{t(perfVerdictKey(issue.verdict))}</Pill>
                        <div className="min-w-0 flex-1">
                          <div className="text-[12.5px]">{t(perfIssueKey(issue))}</div>
                          {issue.value !== null && (
                            <div className="text-[11px] text-ink-3">
                              {t("perf.issue.value", { value: perfIssueValue(issue, units) })}
                            </div>
                          )}
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <div className="space-y-3">
                {PERF_METRICS.map((metric) => (
                  <MetricReportCard key={metric.key} metric={metric} session={session} units={units} t={t} />
                ))}
              </div>
            </>
          ) : (
            <p className="empty faint">{loading ? "…" : t("state.noData")}</p>
          )}
        </div>

        <footer className="flex min-h-[52px] items-center justify-end gap-2 border-t border-input px-5 py-2.5">
          <Button variant="danger" onClick={() => onDelete(shown.summary.id)}>
            {t("perf.report.delete")}
          </Button>
        </footer>
      </SheetContent>
    </Sheet>
  );
}

export function PerfTab() {
  const { snapshot, units, t, pushToast } = useStore();
  const [mode, setMode] = useState<PerfTargetMode>("foreground");
  const [appKey, setAppKey] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<PerfStatus>(EMPTY_STATUS);
  const [sessions, setSessions] = useState<PerfSessionSummary[]>([]);
  const [openId, setOpenId] = useState<string | null>(null);
  const [report, setReport] = useState<PerfSession | null>(null);
  const [loadingReport, setLoadingReport] = useState(false);
  const [liveSamples, setLiveSamples] = useState<PerfSample[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(null);
  const wasActive = useRef<boolean | null>(null);
  const reportSeq = useRef(0);

  // Every reading of the active state has to reach the ref: the stop is only recognised as a
  // stop when the last known state was "recording".
  const noteStatus = useCallback((next: PerfStatus) => {
    wasActive.current = next.active;
    setStatus(next);
  }, []);

  // The picker's rows are memoised and the list around them is too, so the list has to keep
  // its identity across the snapshots that change nothing it draws: a new array every second
  // would undo both memos. `pickerSignature` says what the rows show.
  const appSignature = snapshot ? pickerSignature(snapshot.apps) : "";
  const allApps = useMemo(
    () => perfAppChoices(snapshot?.apps ?? [], ""),
    // Reads the snapshot through the closure on purpose: the signature above is the part of it
    // the rows draw, and it is what the list's identity must follow.
    [appSignature],
  );
  const apps = useMemo(() => perfAppChoices(allApps, query), [allApps, query]);
  const chosen = allApps.find((app) => app.key === appKey) ?? null;

  // Stable across snapshots, so the memoised rows keep their identity; the key travels on the
  // button instead of in a closure.
  const chooseApp = useCallback((event: MouseEvent<HTMLButtonElement>) => {
    const key = event.currentTarget.dataset.key;
    if (key) setAppKey(key);
  }, []);

  const loadReport = useCallback((id: string) => {
    const seq = ++reportSeq.current;
    setLoadingReport(true);
    void ipc.perfReport(id).then((next) => {
      if (seq !== reportSeq.current) return;
      setLoadingReport(false);
      setReport(next);
    });
  }, []);

  const openResult = useCallback(
    (id: string) => {
      setOpenId(id);
      loadReport(id);
    },
    [loadReport],
  );

  const closeResult = useCallback(() => setOpenId(null), []);

  const removeSession = useCallback((id: string) => {
    void ipc.perfDelete(id).then(async () => {
      setSessions(await ipc.perfSessions());
      setOpenId((current) => (current === id ? null : current));
      setReport((current) => (current?.id === id ? null : current));
    });
  }, []);

  useEffect(() => {
    let alive = true;
    void ipc.perfStatus().then((next) => {
      if (alive) noteStatus(next);
    });
    void ipc.perfSessions().then((next) => alive && setSessions(next));
    return () => {
      alive = false;
    };
  }, [noteStatus]);

  // A stop is reported once, by the backend, whether it came from this tab or the shortcut:
  // that event is what refreshes the table, opens the newest result and offers the notice.
  useEffect(() => {
    return subscribe(() =>
      listenEvent<PerfStatus>("perf-status", (next) => {
        setStatus(next);
        const previous = wasActive.current;
        wasActive.current = next.active;
        if (next.active || previous !== true) return;
        void ipc.perfSessions().then((saved) => {
          setSessions(saved);
          const newest = saved[0];
          if (!newest) return;
          openResult(newest.id);
          pushToast({
            tone: "ok",
            text: t("perf.saved"),
            action: { label: t("perf.report.open"), run: () => openResult(newest.id) },
          });
        });
      }),
    );
  }, [openResult, pushToast, t]);

  useEffect(() => {
    if (!status.active) return;
    let alive = true;
    const tick = () => {
      setNow(Date.now());
      void ipc.perfStatus().then((next) => {
        if (!alive) return;
        noteStatus(next);
        const latest = next.latest;
        if (!latest) return;
        setLiveSamples((previous) => {
          const last = previous[previous.length - 1];
          if (last?.atMs === latest.atMs) return previous;
          return [...previous, latest].slice(-MAX_LIVE_SAMPLES);
        });
      });
    };
    tick();
    const timer = window.setInterval(() => {
      if (document.visibilityState !== "hidden") tick();
    }, 1000);
    document.addEventListener("visibilitychange", tick);
    return () => {
      alive = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [status.active, noteStatus]);

  const start = () => {
    void ipc.perfStart(mode, mode === "app" ? appKey : null).then((reason) => {
      if (reason) {
        setError(t("perf.startFailed", { reason }));
        return;
      }
      setError(null);
      setLiveSamples([]);
      void ipc.perfStatus().then(noteStatus);
    });
  };

  const stop = () => {
    void ipc.perfStop().then((result) => {
      if (!result.ok) {
        setError(t("perf.stopFailed", { reason: result.error }));
        return;
      }
      setError(null);
      setLiveSamples([]);
      void ipc.perfStatus().then(noteStatus);
    });
  };

  // One string for every row's meta line, so a row's props stay identical across snapshots
  // unless its own figures changed.
  const processLabel = t("label.processes").toLowerCase();
  const targetFallback = t(status.mode === "app" ? "perf.mode.app" : "perf.mode.foreground");
  const targetName = perfTargetLabel({ mode: status.mode, name: status.targetName }, targetFallback);
  const elapsedSeconds =
    status.active && status.startedAtMs !== null ? Math.max(0, (now - status.startedAtMs) / 1000) : null;
  const elapsed = elapsedSeconds === null ? null : formatDuration(elapsedSeconds, units.locale, { short: true });
  const latest = status.latest;
  const state = status.active ? "live" : sessions.length > 0 ? "saved" : "idle";
  const canStart = mode === "foreground" || appKey !== null;
  const openSummary = openId ? (sessions.find((session) => session.id === openId) ?? null) : null;

  return (
    <div className="stack">
      <Sections
        order={SECTIONS}
        parts={{
          setup: (
            <Card title={t("perf.setup.title")}>
              <div className="perf-deck">
                <div>
                  <div className="perf-mode">
                    <ToggleGroup
                      type="single"
                      value={mode}
                      disabled={status.active}
                      onValueChange={(next) => next && setMode(next as PerfTargetMode)}
                      aria-label={t("perf.mode")}
                    >
                      <ToggleChip value="foreground">{t("perf.mode.foreground")}</ToggleChip>
                      <ToggleChip value="app">{t("perf.mode.app")}</ToggleChip>
                    </ToggleGroup>
                  </div>

                  {mode === "foreground" ? (
                    <p className="perf-note">{t("perf.foregroundHint")}</p>
                  ) : (
                    <div className="perf-picker">
                      <input
                        className="find"
                        type="search"
                        value={query}
                        disabled={status.active}
                        placeholder={t("perf.picker.search")}
                        aria-label={t("perf.appPicker")}
                        spellCheck={false}
                        onChange={(event) => setQuery(event.target.value)}
                      />
                      {allApps.length === 0 ? (
                        <p className="perf-note">{t("perf.noApps")}</p>
                      ) : (
                        <>
                          <p className="perf-picker-count">{t("perf.picker.matches", { count: apps.length })}</p>
                          {apps.length === 0 ? (
                            <p className="perf-note">{t("perf.picker.noMatch")}</p>
                          ) : (
                            <AppPicker
                              apps={apps}
                              selectedKey={appKey}
                              disabled={status.active}
                              processLabel={processLabel}
                              t={t}
                              ariaLabel={t("perf.appPicker")}
                              onSelect={chooseApp}
                            />
                          )}
                        </>
                      )}
                    </div>
                  )}
                </div>

                <div className="perf-session">
                  <p className={`perf-state ${state}`}>
                    <span className="perf-state-dot" aria-hidden="true" />
                    {t(
                      state === "live" ? "perf.state.live" : state === "saved" ? "perf.state.saved" : "perf.state.idle",
                    )}
                  </p>
                  {status.active ? (
                    <>
                      <p className="perf-session-target">{targetName}</p>
                      <span className="perf-elapsed huge num">{elapsed ?? "—"}</span>
                    </>
                  ) : (
                    <>
                      {chosen && (
                        <p className="perf-session-target">
                          {t("perf.target")} {appLabel(chosen, t)}
                        </p>
                      )}
                      <p className="perf-note">{t("perf.idleHint")}</p>
                    </>
                  )}
                  <Button
                    variant={status.active ? "danger" : "primary"}
                    className="perf-action"
                    disabled={!status.active && !canStart}
                    onClick={status.active ? stop : start}
                  >
                    {status.active ? t("perf.stop") : t("perf.start")}
                  </Button>
                  <p className="perf-note">{t("perf.hotkeyHint")}</p>
                  {error && (
                    <p className="perf-error" role="alert">
                      {error}
                    </p>
                  )}
                </div>
              </div>
            </Card>
          ),
          live: status.active ? (
            <Card title={t("perf.live.title")}>
              <div className="perf-strip">
                <span className="perf-strip-target">
                  {t("perf.target")} {targetName}
                </span>
                <span className="perf-strip-elapsed num">
                  {t("perf.elapsed")} {elapsed ?? "—"}
                </span>
              </div>
              {!latest && <p className="perf-note">{t("perf.live.waiting")}</p>}
              <div className="metric-grid">
                {PERF_METRICS.map((metric) => {
                  const value = latest ? metric.read(latest) : null;
                  return (
                    <LiveTile
                      key={metric.key}
                      metric={metric}
                      value={value ?? null}
                      vramBytes={metric.key === "vramPercent" ? (latest?.vramUsedBytes ?? null) : null}
                      series={perfSampleSeries(liveSamples, metric.key)}
                      units={units}
                      t={t}
                    />
                  );
                })}
              </div>
            </Card>
          ) : null,
          history: (
            <Card title={t("perf.history.title")}>
              <p className="perf-table-note">{t("perf.history.saved", { count: sessions.length, max: SESSION_LIMIT })}</p>
              {sessions.length === 0 ? (
                <div className="perf-empty">
                  <p className="perf-empty-title">{t("perf.history.empty")}</p>
                  <p className="perf-empty-body">{t("perf.empty.body", { max: SESSION_LIMIT })}</p>
                  <Button variant="chip" className="border-input" disabled={status.active || !canStart} onClick={start}>
                    {t("perf.empty.start")}
                  </Button>
                </div>
              ) : (
                <table className="perf-table" aria-label={t("perf.history.title")}>
                  <thead>
                    <tr>
                      <th scope="col">{t("perf.table.target")}</th>
                      <th scope="col">{t("perf.table.time")}</th>
                      <th scope="col" className="perf-col-right">
                        {t("perf.table.duration")}
                      </th>
                      <th scope="col">{t("perf.table.verdict")}</th>
                      <th scope="col" className="perf-col-right">
                        {t("perf.table.action")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {sessions.map((session) => {
                      const seconds = sessionSeconds(session);
                      const sessionMode = t(session.mode === "app" ? "perf.mode.app" : "perf.mode.foreground");
                      const name = perfTargetLabel({ mode: session.mode, name: session.targetName }, sessionMode);
                      return (
                        <tr key={session.id} data-open={openId === session.id ? "" : undefined}>
                          <td>
                            <span className="perf-cell-target">{name}</span>
                            <span className="perf-cell-sub">{sessionMode}</span>
                          </td>
                          <td className="perf-cell-time num">{formatTime(session.startedAtMs, units.locale)}</td>
                          <td className="perf-col-right num">
                            {seconds === null ? "—" : formatDuration(seconds, units.locale, { short: true })}
                          </td>
                          <td>
                            <Pill tone={VERDICT_TONE[session.verdict]}>{t(perfVerdictKey(session.verdict))}</Pill>
                          </td>
                          <td>
                            <div className="perf-row-actions">
                              <Button
                                variant="chip"
                                className="border-input"
                                onClick={() => openResult(session.id)}
                              >
                                {t("perf.history.open")}
                              </Button>
                              <Hint label={t("perf.history.delete")}>
                                <Button
                                  variant="mute"
                                  aria-label={t("perf.history.delete")}
                                  onClick={() => removeSession(session.id)}
                                >
                                  <Trash2 aria-hidden className="size-3.5" />
                                </Button>
                              </Hint>
                            </div>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
            </Card>
          ),
        }}
      />
      <ResultSheet
        summary={openSummary}
        report={report}
        loading={loadingReport}
        onClose={closeResult}
        onDelete={removeSession}
      />
    </div>
  );
}
