import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { formatBytes, formatDuration, formatTime } from "../lib/format";
import { ipc, listenEvent } from "../lib/ipc";
import { subscribe } from "../lib/subscribe";
import {
  PERF_METRICS,
  formatPerfStat,
  formatPerfValue,
  perfHeadlineKey,
  perfIssueKey,
  perfIssueValue,
  perfSampleSeries,
  perfSeries,
  perfTargetLabel,
  perfVerdictKey,
  type PerfMetric,
} from "../lib/perf";
import { useStore } from "../lib/store";
import type { PerfReport, PerfSample, PerfSession, PerfSessionSummary, PerfStatus, PerfTargetMode, PerfVerdict } from "../lib/types";
import type { Translate } from "../i18n";
import type { UnitPrefs } from "../lib/format";
import { Button } from "./ui/button";
import { Card, Chart, Pill, Sections, Stat } from "./ui";
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

function unavailableReason(metric: PerfMetric, t: Translate): string {
  if (metric.key === "fps") return t("perf.fpsUnavailable");
  if (metric.key === "cpuTempC") return t("perf.cpuTempUnavailable");
  return t("state.unavailable");
}

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
      <div className="grid grid-cols-5 gap-2">
        {figures.map(([label, value]) => (
          <Stat key={label} label={label} value={formatPerfValue(value, metric.unit, units)} />
        ))}
      </div>
      {vramBytes && (
        <p className="hint faint">{t("perf.vram.used", { bytes: formatPerfStat(vramBytes, "bytes", units) })}</p>
      )}
      <Chart values={perfSeries(session, metric.key)} ceiling={metric.ceiling} color={metric.tone} height={80} />
    </Card>
  );
}

function ReportView({
  session,
  units,
  t,
  onBack,
}: {
  session: PerfSession;
  units: UnitPrefs;
  t: Translate;
  onBack: () => void;
}) {
  const report: PerfReport | null = session.report;
  if (!report) return <p className="empty faint">{t("state.noData")}</p>;
  const duration = formatDuration(report.durationSeconds, units.locale);
  const appCpu = formatPerfStat(report.stats.find((entry) => entry.key === "appCpuPercent"), "percent", units);
  const appGpu = formatPerfStat(report.stats.find((entry) => entry.key === "appGpuPercent"), "percent", units);
  return (
    <div className="stack">
      <Card
        title={t("perf.report.title")}
        tone={VERDICT_TONE[report.verdict] === "danger" ? "var(--m-danger)" : undefined}
        right={<Button variant="chip" onClick={onBack}>{t("perf.report.back")}</Button>}
      >
        <div className="hero-figure">
          <Pill tone={VERDICT_TONE[report.verdict]}>{t(perfVerdictKey(report.verdict))}</Pill>
          <span>{t(perfHeadlineKey(report))}</span>
        </div>
        <Stat label={t("perf.report.duration")} value={duration} />
        <p className="hint faint">{t("perf.report.samples", { count: report.sampleCount, duration })}</p>
        <div className="legend">
          <span>{t("perf.metric.appCpu")} {appCpu}</span>
          <span>{t("perf.metric.appGpu")} {appGpu}</span>
        </div>
      </Card>

      <Card title={t("perf.report.findings")}>
        {report.issues.length === 0 ? (
          <p className="empty faint">{t("perf.report.noFindings")}</p>
        ) : (
          <div className="breakdown">
            {report.issues.map((issue) => (
              <div key={`${issue.kind}:${issue.metric}`} className="stat">
                <span className="stat-label">{t(perfVerdictKey(issue.verdict))}</span>
                <span>{t(perfIssueKey(issue))}</span>
                {issue.value !== null && (
                  <span className="stat-sub">{t("perf.issue.value", { value: perfIssueValue(issue, units) })}</span>
                )}
              </div>
            ))}
          </div>
        )}
      </Card>

      {PERF_METRICS.map((metric) => (
        <MetricReportCard key={metric.key} metric={metric} session={session} units={units} t={t} />
      ))}
    </div>
  );
}

export function PerfTab() {
  const { snapshot, units, t, pushToast } = useStore();
  const [mode, setMode] = useState<PerfTargetMode>("foreground");
  const [appKey, setAppKey] = useState<string | null>(null);
  const [status, setStatus] = useState<PerfStatus>(() => ({
    active: false,
    sessionId: null,
    targetName: "",
    mode: "foreground",
    currentAppKey: null,
    sampleCount: 0,
    startedAtMs: null,
    latest: null,
  }));
  const [sessions, setSessions] = useState<PerfSessionSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [report, setReport] = useState<PerfSession | null>(null);
  const [liveSamples, setLiveSamples] = useState<PerfSample[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(null);
  const selectSeq = useRef(0);
  const wasActive = useRef<boolean | null>(null);
  const reportReady = useRef<(id: string) => void>(() => {});
  const toastOpen = useRef<(id: string) => void>(() => {});

  const apps = useMemo(
    () => [...(snapshot?.apps ?? [])].filter((app) => app.name.trim()).sort((a, b) => a.name.localeCompare(b.name)),
    [snapshot],
  );

  useEffect(() => {
    let alive = true;
    void ipc.perfStatus().then((next) => {
      if (!alive) return;
      wasActive.current = next.active;
      setStatus(next);
    });
    void ipc.perfSessions().then((next) => alive && setSessions(next));
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    return subscribe(() =>
      listenEvent<PerfStatus>("perf-status", (next) => {
        setStatus(next);
        const previous = wasActive.current;
        wasActive.current = next.active;
        if (previous !== true || next.active) return;
        void ipc.perfSessions().then((saved) => {
          setSessions(saved);
          const newest = saved[0];
          const openResult = () => {
            if (newest) toastOpen.current(newest.id);
          };
          pushToast({
            tone: "ok",
            text: t("perf.saved"),
            action: newest ? { label: t("perf.report.open"), run: openResult } : undefined,
          });
        });
      }),
    );
  }, [pushToast, t]);

  useEffect(() => {
    if (!status.active) return;
    let alive = true;
    const tick = () => {
      setNow(Date.now());
      void ipc.perfStatus().then((next) => {
        if (!alive) return;
        setStatus(next);
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
  }, [status.active]);

  const start = () => {
    void ipc.perfStart(mode, mode === "app" ? appKey : null).then((reason) => {
      if (reason) {
        setError(t("perf.startFailed", { reason }));
        return;
      }
      setError(null);
      setLiveSamples([]);
      void ipc.perfStatus().then((next) => setStatus(next));
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
      void ipc.perfStatus().then((next) => setStatus(next));
      void ipc.perfSessions().then((next) => {
        setSessions(next);
        if (result.value) {
          const id = result.value.sessionId;
          setSelectedId(id);
          const seq = ++selectSeq.current;
          void ipc.perfReport(id).then((next) => seq === selectSeq.current && setReport(next));
        }
      });
    });
  };

  const openSession = (id: string) => {
    reportReady.current(id);
    setSelectedId(id);
  };

  toastOpen.current = openSession;

  reportReady.current = (id: string) => {
    const seq = ++selectSeq.current;
    void ipc.perfReport(id).then((next) => seq === selectSeq.current && setReport(next));
  };

  const removeSession = (id: string) => {
    void ipc.perfDelete(id).then(async () => {
      setSessions(await ipc.perfSessions());
      if (selectedId === id) {
        setSelectedId(null);
        setReport(null);
      }
    });
  };

  const targetFallback = t(status.mode === "app" ? "perf.mode.app" : "perf.mode.foreground");
  const targetName = perfTargetLabel({ mode: status.mode, name: status.targetName }, targetFallback);
  const elapsed =
    status.startedAtMs === null
      ? "—"
      : formatDuration(Math.max(0, (now - status.startedAtMs) / 1000), units.locale, { short: true });
  const latest = status.latest;

  return (
    <div className="stack">
      <Sections
        order={SECTIONS}
        parts={{
          setup: (
            <Card title={t("perf.setup.title")}>
              <div className="table-tools">
                <ToggleGroup
                  type="single"
                  value={mode}
                  onValueChange={(next) => next && setMode(next as PerfTargetMode)}
                  aria-label={t("perf.mode")}
                >
                  <ToggleChip value="app">{t("perf.mode.app")}</ToggleChip>
                  <ToggleChip value="foreground">{t("perf.mode.foreground")}</ToggleChip>
                </ToggleGroup>
                {mode === "app" && (
                  <select
                    className="find"
                    aria-label={t("perf.appPicker")}
                    value={appKey ?? ""}
                    onChange={(event) => setAppKey(event.target.value || null)}
                  >
                    <option value="">{t("perf.appPicker")}</option>
                    {apps.map((app) => (
                      <option key={app.key} value={app.key}>
                        {app.name}
                      </option>
                    ))}
                  </select>
                )}
                <Button variant="primary" disabled={status.active || (mode === "app" && !appKey)} onClick={start}>
                  {t("perf.start")}
                </Button>
                <Button variant="danger" disabled={!status.active} onClick={stop}>
                  {t("perf.stop")}
                </Button>
                {status.active && (
                  <span className="faint">
                    {t("perf.target")} {targetName} · {t("perf.elapsed")} {elapsed}
                  </span>
                )}
              </div>
              {mode === "app" && apps.length === 0 && <p className="hint faint">{t("perf.noApps")}</p>}
              <p className="hint faint">{t("perf.hotkeyHint")}</p>
              {error && (
                <p className="hint" role="alert">
                  {error}
                </p>
              )}
            </Card>
          ),
          live: status.active ? (
            <Card title={t("perf.live.title")} right={<span className="faint num">{targetName} · {elapsed}</span>}>
              {!latest && <p className="hint faint">{t("perf.live.empty")}</p>}
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
            <div className="stack">
              <Card title={t("perf.history.title")}>
                <p className="hint faint">
                  {t("perf.history.saved", { count: sessions.length, max: SESSION_LIMIT })}
                </p>
                {sessions.length === 0 ? (
                  <p className="empty faint">{t("perf.history.empty")}</p>
                ) : (
                  <div className="rows">
                    {sessions.map((session) => {
                      const seconds =
                        session.endedAtMs === null ? null : (session.endedAtMs - session.startedAtMs) / 1000;
                      return (
                        <div
                          key={session.id}
                          className={`row clickable ${selectedId === session.id ? "pinned" : ""}`}
                          role="button"
                          tabIndex={0}
                          onClick={() => openSession(session.id)}
                          onKeyDown={(event) => event.key === "Enter" && openSession(session.id)}
                        >
                          <span />
                          <div className="row-main">
                            <span className="row-name">{session.targetName || targetFallback}</span>
                            <span className="row-sub">
                              {formatTime(session.startedAtMs, units.locale)} ·{" "}
                              {t(session.mode === "app" ? "perf.mode.app" : "perf.mode.foreground")}
                            </span>
                            <Hint label={t("perf.history.open")}>
                              <Button
                                variant="chip"
                                className="border-input mt-1 self-start"
                                aria-label={t("perf.history.open")}
                                onClick={(event) => {
                                  event.stopPropagation();
                                  openSession(session.id);
                                }}
                              >
                                {t("perf.history.open")}
                              </Button>
                            </Hint>
                          </div>
                          <Pill tone={VERDICT_TONE[session.verdict]}>{t(perfVerdictKey(session.verdict))}</Pill>
                          <span className="row-value num">
                            {seconds === null ? "—" : formatDuration(seconds, units.locale, { short: true })}
                          </span>
                          <Hint label={t("perf.history.delete")}>
                            <Button
                              variant="mute"
                              aria-label={t("perf.history.delete")}
                              onClick={(event) => {
                                event.stopPropagation();
                                removeSession(session.id);
                              }}
                            >
                              ×
                            </Button>
                          </Hint>
                        </div>
                      );
                    })}
                  </div>
                )}
              </Card>
              {selectedId && report && (
                <ReportView session={report} units={units} t={t} onBack={() => setSelectedId(null)} />
              )}
            </div>
          ),
        }}
      />
    </div>
  );
}
