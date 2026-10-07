import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ipc } from "../lib/ipc";
import { useStore } from "../lib/store";
import { formatBytes, formatPercent, formatRate, cpuDisplay } from "../lib/format";
import type { UnitPrefs } from "../lib/format";
import type { Translate } from "../i18n";
import type { AppInfo, Availability } from "../lib/types";

export function Card({
  title,
  right,
  children,
  className = "",
  tone,
}: {
  title?: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  className?: string;
  tone?: string;
}) {
  return (
    <section className={`card ${className}`} style={tone ? { ["--tone" as string]: tone } : undefined}>
      {(title || right) && (
        <header className="card-head">
          {title && <h3>{title}</h3>}
          {right && <div className="card-tools">{right}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

export function Stat({
  label,
  value,
  sub,
  color,
}: {
  label: string;
  value: string;
  sub?: string;
  color?: string;
}) {
  return (
    <div className="stat">
      <span className="stat-label">{label}</span>
      <span className="stat-value num" style={color ? { color } : undefined}>
        {value}
      </span>
      {sub && <span className="stat-sub">{sub}</span>}
    </div>
  );
}

export function Chart({
  values,
  max,
  color,
  variant = "area",
  height = 56,
  invertAxis = false,
}: {
  values: number[];
  max?: number;
  color: string;
  variant?: "area" | "line" | "bars";
  height?: number;
  invertAxis?: boolean;
}) {
  const width = 240;
  const data = values.length > 0 ? values : [0];
  const top = Math.max(max ?? 0, ...data, 1);
  const normalized = data.map((v) => {
    const r = Math.min(1, v / top);
    return invertAxis ? 1 - r : r;
  });

  if (variant === "bars") {
    return (
      <div className="bars" style={{ height }}>
        {normalized.slice(-56).map((ratio, i) => (
          <span
            key={i}
            className="bar-segment"
            style={{ height: `${Math.max(6, ratio * 100)}%`, background: color }}
          />
        ))}
      </div>
    );
  }

  const step = width / Math.max(1, normalized.length - 1);
  const points = normalized.map((r, i) => `${(i * step).toFixed(2)},${(height - r * height).toFixed(2)}`);
  const line = `M${points.join(" L")}`;
  const area = `${line} L${width},${height} L0,${height} Z`;
  return (
    <svg className="chart" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img">
      {variant === "area" && <path d={area} className="chart-area" fill={color} opacity={0.16} />}
      <path d={line} fill="none" stroke={color} strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function CoreGrid({ values, units }: { values: number[]; units: UnitPrefs }) {
  return (
    <div className="core-grid">
      {values.map((value, i) => (
        <div key={i} className="core" title={`${i}: ${Math.round(value)}%`}>
          <div className="core-track">
            <div className="core-fill" style={{ height: `${Math.min(100, value)}%` }} />
          </div>
          <span className="core-index">{i}</span>
        </div>
      ))}
      <p className="hint faint">
        {formatPercent(values.reduce((a, b) => a + b, 0) / Math.max(1, values.length), units.locale, 0)}{" "}
        average across {values.length} logical cores
      </p>
    </div>
  );
}

export function Pill({ children, tone }: { children: ReactNode; tone?: string }) {
  return <span className={`pill ${tone ?? ""}`}>{children}</span>;
}

export function Meter({ ratio, color }: { ratio: number; color: string }) {
  return (
    <div className="meter">
      <div className="meter-fill" style={{ width: `${Math.min(100, Math.max(0, ratio * 100))}%`, background: color }} />
    </div>
  );
}

export function AvailabilityNote({ state, t }: { state: Availability; t: Translate }) {
  if (state === "ok") return null;
  const key =
    state === "needsElevation"
      ? "state.needsElevation"
      : state === "partial"
        ? "state.partial"
        : "state.unavailable";
  return (
    <p className="unavailable">
      <span className="dot" />
      {t(key)}
    </p>
  );
}

const KIND_TONE: Record<AppInfo["kind"], string> = {
  app: "",
  background: "faint",
  service: "faint",
  system: "faint",
  packaged: "",
};

export function Monogram({ name, seed }: { name: string; seed: string }) {
  const hue = useMemo(() => {
    let h = 0;
    for (const c of seed) h = (h * 31 + c.charCodeAt(0)) % 360;
    return h;
  }, [seed]);
  return (
    <span
      className="mono-avatar"
      style={{ background: `hsl(${hue} 42% 34%)`, color: `hsl(${hue} 70% 86%)` }}
    >
      {(name.trim()[0] ?? "?").toUpperCase()}
    </span>
  );
}

export type AppMetric = "cpu" | "memory" | "disk" | "network" | "connections";

export function appValue(app: AppInfo, metric: AppMetric): number {
  switch (metric) {
    case "cpu":
      return app.cpuPercent;
    case "memory":
      return app.memoryBytes;
    case "disk":
      return app.readBytesPerSecond + app.writeBytesPerSecond;
    case "network":
      return app.connections;
    case "connections":
      return app.connections;
  }
}

export function AppTable({
  apps,
  metric,
  onMetricChange,
  units,
  t,
  cap,
  seedQuery,
}: {
  apps: AppInfo[];
  metric: AppMetric;
  onMetricChange: (metric: AppMetric) => void;
  units: UnitPrefs;
  t: Translate;
  cap?: number;
  seedQuery?: string;
}) {
  const { requestQuit } = useStore();
  const [query, setQuery] = useState(seedQuery ?? "");
  const [pinned, setPinned] = useState<string[]>([]);
  useEffect(() => {
    if (seedQuery !== undefined) setQuery(seedQuery);
  }, [seedQuery]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const list = apps.filter(
      (a) =>
        !needle ||
        a.name.toLowerCase().includes(needle) ||
        a.exePath.toLowerCase().includes(needle) ||
        a.pids.some((p) => String(p) === needle),
    );
    const sorted = [...list].sort((a, b) => {
      const pa = pinned.indexOf(a.key);
      const pb = pinned.indexOf(b.key);
      if (pa !== -1 || pb !== -1) return (pa === -1 ? 1 : pa) - (pb === -1 ? 1 : pb);
      return appValue(b, metric) - appValue(a, metric);
    });
    return cap ? sorted.slice(0, cap) : sorted;
  }, [apps, query, metric, cap, pinned]);

  const peak = Math.max(1, ...filtered.map((a) => appValue(a, metric)));

  const format = (app: AppInfo) => {
    switch (metric) {
      case "cpu":
        return formatPercent(cpuDisplay(app.cpuPercent, units), units.locale, app.cpuPercent >= 10 ? 0 : 1);
      case "memory":
        return formatBytes(app.memoryBytes, units);
      case "disk":
        return formatRate(app.readBytesPerSecond + app.writeBytesPerSecond, units, "storage");
      case "network":
      case "connections":
        return String(app.connections);
    }
  };

  return (
    <div className="app-table">
      <div className="table-tools">
        <input
          className="find"
          placeholder={t("action.searchPlaceholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          spellCheck={false}
        />
        <div className="sort-group" role="group" aria-label={t("list.sortBy")}>
          {(["cpu", "memory", "disk", "connections"] as AppMetric[]).map((key) => (
            <button
              key={key}
              className={`chip ${metric === key ? "on" : ""}`}
              onClick={() => onMetricChange(key)}
            >
              {t(`list.${key === "connections" ? "network" : key}`)}
            </button>
          ))}
        </div>
      </div>
      <div className="rows">
        {filtered.map((app) => (
          <div
            key={app.key}
            className={`row ${pinned.includes(app.key) ? "pinned" : ""}`}
            onContextMenu={(e) => {
              e.preventDefault();
              setPinned((p) => (p.includes(app.key) ? p.filter((k) => k !== app.key) : [...p, app.key]));
            }}
            title={app.windows.map((w) => w.title).join("\u000a") || app.exePath}
          >
            <Monogram name={app.name} seed={app.key} />
            <div className="row-main">
              <span className="row-name">{app.name}</span>
              <span className={`row-sub ${KIND_TONE[app.kind]}`}>
                {t(`kind.${app.kind}`)}
                {app.processCount > 1 ? ` · ${app.processCount} ${t("label.processes").toLowerCase()}` : ""}
                {app.listeningPorts.length > 0
                  ? ` · ${app.listeningPorts.slice(0, 4).join(", ")}`
                  : ""}
              </span>
            </div>
            <div className="row-meter">
              <Meter ratio={appValue(app, metric) / peak} color="var(--accent)" />
            </div>
            <span className="row-value num">{format(app)}</span>
            <button className="row-quit" onClick={() => requestQuit(app)} aria-label={t("action.quitApp")}>
              ×
            </button>
          </div>
        ))}
        {filtered.length === 0 && <p className="empty faint">{t("state.noData")}</p>}
      </div>
    </div>
  );
}

/**
 * Nothing closes without being asked first, and force quit is offered separately rather
 * than taken for granted — the same two-step the Mac version uses.
 */
export function QuitDialog() {
  const { pendingQuit, clearQuit, t } = useStore();
  if (!pendingQuit) return null;
  const app = pendingQuit;
  return (
    <div
      className="overlay"
      onClick={clearQuit}
      role="dialog"
      aria-modal="true"
      aria-label={t("action.confirmQuit", { name: app.name })}
    >
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h3>{t("action.confirmQuit", { name: app.name })}</h3>
        <p className="muted">{t("action.confirmQuitBody", { count: app.processCount })}</p>
        <p className="faint dialog-sub">
          {app.windows
            .filter((w) => w.visible)
            .slice(0, 3)
            .map((w) => w.title)
            .join(" · ")}
        </p>
        <div className="dialog-actions">
          <button className="chip" onClick={clearQuit}>
            {t("action.cancel")}
          </button>
          <span className="spacer" />
          <button
            className="danger"
            onClick={() => {
              void ipc.quitApp(app.key, true);
              clearQuit();
            }}
          >
            {t("action.forceQuit")}
          </button>
          <button
            className="primary"
            onClick={() => {
              void ipc.quitApp(app.key, false);
              clearQuit();
            }}
          >
            {t("action.quitApp")}
          </button>
        </div>
      </div>
    </div>
  );
}
