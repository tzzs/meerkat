import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ipc } from "../lib/ipc";
import { Badge, type BadgeProps } from "./ui/badge";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogActions,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Card as UICard, CardHeader, CardTitle, CardTools } from "./ui/card";
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import { Hint } from "./ui/tooltip";
import { useStore } from "../lib/store";
import { formatBytes, formatPercent, formatRate, cpuDisplay } from "../lib/format";
import type { UnitPrefs } from "../lib/format";
import type { Translate } from "../i18n";
import type { AppInfo, Availability } from "../lib/types";
import { UNREADABLE_GROUP } from "../lib/types";
import { appHint, appLabel, serviceNote } from "../lib/apps";

/**
 * The card every tab is built from. Its parts are the shadcn primitives in ./ui/card; this
 * is the short form the tabs use, because 36 call sites pass a title and an optional control
 * strip rather than composing the header themselves.
 */
export function Card({
  title,
  right,
  children,
  className,
  tone,
}: {
  title?: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  className?: string;
  tone?: string;
}) {
  return (
    <UICard className={className} tone={tone}>
      {(title || right) && (
        <CardHeader>
          {title && <CardTitle>{title}</CardTitle>}
          {right && <CardTools>{right}</CardTools>}
        </CardHeader>
      )}
      {children}
    </UICard>
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

/** Rounds up to the next 1/2/5 step, so an axis ceiling is a number a person can read. */
function niceCeiling(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1;
  const base = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 5, 10].find((s) => value <= s * base) ?? 10;
  return step * base;
}

/**
 * A chart ceiling that rises at once and falls slowly.
 *
 * Rescaling to the visible maximum on every sample makes the entire curve change height each
 * second even when the data did not move, which is what reads as the graph bouncing. Holding a
 * rounded ceiling keeps the shape steady, and a new peak still raises it immediately because
 * understating a spike would be a lie about the measurement.
 */
function useHeldCeiling(observed: number, holdSamples = 6): number {
  const [ceiling, setCeiling] = useState(() => niceCeiling(observed));
  const below = useRef(0);

  useEffect(() => {
    const next = niceCeiling(observed);
    if (next > ceiling) {
      below.current = 0;
      setCeiling(next);
      return;
    }
    if (next === ceiling) {
      below.current = 0;
      return;
    }
    below.current += 1;
    if (below.current >= holdSamples) {
      below.current = 0;
      setCeiling(next);
    }
  }, [observed, ceiling, holdSamples]);

  return ceiling;
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
  const top = useHeldCeiling(Math.max(max ?? 0, ...data));
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
  const { t } = useStore();
  return (
    <div className="core-grid">
      {values.map((value, i) => (
        <Hint key={i} label={`${i}: ${Math.round(value)}%`} delayDuration={0}>
          <div className="core">
            {/* Height rather than a scale: twenty bars cost nothing to lay out, and scaling
                one would squash its gradient along with it. */}
            <div className="core-track">
              <div className="core-fill" style={{ height: `${Math.min(100, Math.max(0, value))}%` }} />
            </div>
            <span className="core-index">{i}</span>
          </div>
        </Hint>
      ))}
      <p className="hint faint">
        {t("cpu.perCoreAverage", {
          average: formatPercent(
            values.reduce((a, b) => a + b, 0) / Math.max(1, values.length),
            units.locale,
            0,
          ),
          cores: values.length,
        })}
      </p>
    </div>
  );
}

const PILL_VARIANT: Record<string, NonNullable<BadgeProps["variant"]>> = {
  "": "default",
  accent: "accent",
  faint: "faint",
  danger: "danger",
};

export function Pill({
  children,
  tone,
  className,
}: {
  children: ReactNode;
  tone?: string;
  className?: string;
}) {
  return (
    <Badge variant={PILL_VARIANT[tone ?? ""]} className={className}>
      {children}
    </Badge>
  );
}

/**
 * Eases a number towards its latest reading instead of snapping to it.
 *
 * The sampler reports once a second, and a headline that jumps by nine every second reads as
 * a glitch rather than as a machine that is working. Each new reading is walked to over a few
 * hundred milliseconds from wherever the previous animation had got to, so a change that
 * arrives mid-flight stays smooth instead of restarting.
 */
export function useTweenedNumber(value: number, ms = 420, ease = true): number {
  const [shown, setShown] = useState(value);
  const position = useRef(value);

  useEffect(() => {
    if (value === position.current) return;
    if (!ease || window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      position.current = value;
      setShown(value);
      return;
    }
    const from = position.current;
    const started = performance.now();
    let frame = 0;
    const step = (now: number) => {
      const t = Math.min(1, (now - started) / ms);
      const eased = 1 - Math.pow(1 - t, 3);
      position.current = from + (value - from) * eased;
      setShown(position.current);
      if (t < 1) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [value, ms]);

  return shown;
}

/**
 * A headline figure that moves. `null` is the honest "no reading" case and stays a dash:
 * easing towards zero would claim the machine went quiet.
 *
 * `ease={false}` is for rates. A throughput has no continuous value to walk towards — the
 * number between two one-second samples is a figure that was never measured — so easing it
 * manufactures motion out of noise. State quantities (percentage, bytes in use, degrees) keep
 * the easing because those really do change smoothly.
 */
export function Tween({
  value,
  format,
  className,
  style,
  ease = true,
}: {
  value: number | null;
  format: (n: number) => string;
  className?: string;
  style?: CSSProperties;
  ease?: boolean;
}) {
  const shown = useTweenedNumber(value ?? 0, 420, ease);
  return (
    <span className={className} style={style}>
      {value === null ? "—" : format(shown)}
    </span>
  );
}

export function Meter({ ratio, color }: { ratio: number; color: string }) {
  const clamped = Math.min(1, Math.max(0, Number.isFinite(ratio) ? ratio : 0));
  return (
    <div className="meter">
      <div className="meter-fill" style={{ transform: `scaleX(${clamped})`, background: color }} />
    </div>
  );
}

export function AvailabilityNote({
  state,
  t,
  elevate = false,
}: {
  state: Availability;
  t: Translate;
  /** Whether elevation is the thing that could plausibly change this. Not every gap is a
      permission gap, and offering the button where it would do nothing is worse than not
      offering it. */
  elevate?: boolean;
}) {
  const [refusal, setRefusal] = useState<string | null>(null);
  if (state === "ok") return null;
  const key =
    state === "needsElevation"
      ? "state.needsElevation"
      : state === "partial"
        ? "state.partial"
        : "state.unavailable";
  const canAsk = elevate && (state === "needsElevation" || state === "partial") && ipc.enabled;
  return (
    <div className="unavailable">
      <span className="dot" />
      <span className="unavailable-text">{t(key)}</span>
      {canAsk && (
        <span className="unavailable-action">
          <Button
            variant="chip"
            onClick={() => {
              void ipc.relaunchElevated().then((reason) => {
                if (reason) setRefusal(reason === "cancelled" ? t("state.elevateCancelled") : reason);
              });
            }}
          >
            {t("action.elevateRestart")}
          </Button>
          <span className="faint">
            {refusal ?? t("state.elevateHelp")}
          </span>
        </span>
      )}
    </div>
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

/** The leading column of a row: 22 CSS px, whether it holds an icon or an initial. */
export const AVATAR_PX = 22;

const iconCache = new Map<string, string | null>();
const iconRequests = new Map<string, Promise<string | null>>();

/** One ask per executable per size, shared by every row that shows the same program. */
function loadIcon(exePath: string, size: number): Promise<string | null> {
  const key = `${exePath}@${size}`;
  const running = iconRequests.get(key);
  if (running) return running;
  const request = ipc.appIcon(exePath, size).then((url) => {
    iconCache.set(key, url);
    iconRequests.delete(key);
    return url;
  });
  iconRequests.set(key, request);
  return request;
}

/**
 * A program's own icon, with the initial behind it.
 *
 * The fallback is not decoration: a process whose executable cannot be read has no icon to
 * ask for, and a row with an empty leading column reads as a row that failed to load.
 */
export function AppIcon({
  name,
  seed,
  exePath,
}: {
  name: string;
  seed: string;
  exePath?: string;
}) {
  // Asked for at the device scale, or a 22px icon on a 150% display is resampled and soft.
  const size = Math.max(AVATAR_PX, Math.round(AVATAR_PX * (window.devicePixelRatio || 1)));
  const key = exePath ? `${exePath}@${size}` : "";
  const [src, setSrc] = useState<string | null>(() => (key ? iconCache.get(key) ?? null : null));

  useEffect(() => {
    if (!key || !exePath) {
      setSrc(null);
      return;
    }
    const known = iconCache.get(key);
    if (known !== undefined) {
      setSrc(known);
      return;
    }
    let current = true;
    void loadIcon(exePath, size).then((url) => {
      if (current) setSrc(url);
    });
    return () => {
      current = false;
    };
  }, [key, exePath, size]);

  if (!src) return <Monogram name={name} seed={seed} />;
  return (
    <img
      className="app-icon"
      src={src}
      width={AVATAR_PX}
      height={AVATAR_PX}
      alt=""
      draggable={false}
    />
  );
}

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

  // The list scrolls with the page rather than in its own box, so the virtualiser needs
  // the distance between the scroller's top and the list's top. That distance is only
  // stable while everything above the list keeps its height, and the cards above do move
  // (an insight row appears, a graph reflows when the window resizes), so the margin is
  // re-read after every render and on scroll as well as on resize. A stale margin would
  // shift the whole rendered block off the rows the user is actually looking at.
  const listRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLElement | null>(null);
  const [scrollEl, setScrollEl] = useState<HTMLElement | null>(null);
  const [scrollMargin, setScrollMargin] = useState(0);

  const measure = useCallback(() => {
    const list = listRef.current;
    const scroller = scrollRef.current;
    if (!list || !scroller) return;
    const next =
      list.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    // Setting the same number back into state would re-render, which would measure again.
    setScrollMargin((prev) => (Math.abs(prev - next) < 0.5 ? prev : Math.round(next)));
  }, []);

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const scroller = (list.closest(".content") as HTMLElement | null) ?? null;
    scrollRef.current = scroller;
    setScrollEl(scroller);
    measure();
  }, [measure]);

  useEffect(() => {
    const scroller = scrollEl;
    if (!scroller) return;
    let frame = 0;
    const onScroll = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        measure();
      });
    };
    const observer = new ResizeObserver(measure);
    if (listRef.current) observer.observe(listRef.current);
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      if (frame) cancelAnimationFrame(frame);
      scroller.removeEventListener("scroll", onScroll);
      observer.disconnect();
    };
  }, [scrollEl, measure]);

  // Everything above the list can change height as the snapshot updates, and that does not
  // resize the list itself, so the margin is re-read after each render rather than cached.
  useEffect(measure);

  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollEl,
    // Measured in the browser: an application row is 47px tall (7px of padding twice over
    // two lines of text). The estimate is what sizes the scrollbar before the first row is
    // measured, so an off-by-ten guess shows as a blank strip while scrolling fast.
    estimateSize: () => 47,
    overscan: 8,
    scrollMargin,
  });

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
        <ToggleGroup
          type="single"
          className="ml-auto"
          value={metric}
          onValueChange={(next) => {
            if (next) onMetricChange(next as AppMetric);
          }}
          aria-label={t("list.sortBy")}
        >
          {(["cpu", "memory", "disk", "connections"] as AppMetric[]).map((key) => (
            <ToggleChip key={key} value={key}>
              {t(`list.${key === "connections" ? "network" : key}`)}
            </ToggleChip>
          ))}
        </ToggleGroup>
      </div>
      <div
        className="rows"
        ref={listRef}
        data-count={filtered.length}
        style={filtered.length > 0 ? { position: "relative", height: virtualizer.getTotalSize() } : undefined}
      >
        {virtualizer.getVirtualItems().map((item) => {
          const app = filtered[item.index];
          if (!app) return null;
          // The synthetic row is a count of processes that need administrative rights to open,
          // not an application: it has no figures, and it must never offer to close anything.
          const unreadable = app.key === UNREADABLE_GROUP;
          const label = appLabel(app, t);
          const services = serviceNote(app, t);
          return (
            <Hint
              key={app.key}
              className="max-w-[420px] whitespace-pre-line text-left"
              label={appHint(app, t)}
            >
              <div
                data-index={item.index}
                ref={virtualizer.measureElement}
                className={`row ${pinned.includes(app.key) ? "pinned" : ""}`}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  right: 0,
                  transform: `translateY(${item.start - scrollMargin}px)`,
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setPinned((p) => (p.includes(app.key) ? p.filter((k) => k !== app.key) : [...p, app.key]));
                }}
              >
                <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
                <div className="row-main">
                  <span className="row-name">{label}</span>
                  <span className={`row-sub ${KIND_TONE[app.kind]}`}>
                    {t(`kind.${app.kind}`)}
                    {app.processCount > 1 ? ` · ${app.processCount} ${t("label.processes").toLowerCase()}` : ""}
                    {services ? ` · ${services}` : ""}
                    {app.listeningPorts.length > 0
                      ? ` · ${app.listeningPorts.slice(0, 4).join(", ")}`
                      : ""}
                  </span>
                </div>
                <div className="row-meter">
                  <Meter ratio={unreadable ? 0 : appValue(app, metric) / peak} color="var(--accent)" />
                </div>
                <span className="row-value num">{unreadable ? "—" : format(app)}</span>
                {/* Windows will not let this user end a service or another account's process,
                    so those rows do not offer it; the hover text says why. */}
                {app.canQuit && !unreadable && (
                  <button className="row-quit" onClick={() => requestQuit(app)} aria-label={t("action.quitApp")}>
                    ×
                  </button>
                )}
              </div>
            </Hint>
          );
        })}
        {filtered.length === 0 && <p className="empty faint">{t("state.noData")}</p>}
      </div>
    </div>
  );
}

/**
 * Nothing closes without being asked first, and force quit is offered separately rather
 * than taken for granted — the same two-step the Mac version uses.
 *
 * Radix owns the behaviour: focus moves in and back out, Escape and the backdrop dismiss,
 * the rest of the page is inert while it is up, and the node stays mounted for the exit
 * animation. That last part is why the previous value is kept in a ref — the store has
 * already forgotten which app was asked about by the time the animation is halfway done,
 * and a dialog that empties itself while sliding away is worse than no animation.
 */
export function QuitDialog() {
  const { pendingQuit, clearQuit, t } = useStore();
  const lastAskedAbout = useRef<AppInfo | null>(null);
  if (pendingQuit) lastAskedAbout.current = pendingQuit;
  const app = pendingQuit ?? lastAskedAbout.current;
  if (!app) return null;
  const title = t("action.confirmQuit", { name: appLabel(app, t) });
  return (
    <Dialog open={Boolean(pendingQuit)} onOpenChange={(next) => !next && clearQuit()}>
      <DialogContent label={title}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className="text-muted-foreground">
            {t("action.confirmQuitBody", { count: app.processCount })}
          </DialogDescription>
          <DialogDescription className="mt-1.5 truncate text-[11.5px] text-ink-3">
            {app.windows
              .filter((w) => w.visible)
              .slice(0, 3)
              .map((w) => w.title)
              .join(" · ")}
          </DialogDescription>
        </DialogHeader>
        <DialogActions>
          {/* The safe action takes focus first, so Enter on an open dialog cancels it. */}
          <DialogClose>
            <Button variant="chip" data-autofocus>
              {t("action.cancel")}
            </Button>
          </DialogClose>
          <span className="spacer" />
          <Button
            variant="danger"
            onClick={() => {
              void ipc.quitApp(app.key, true);
              clearQuit();
            }}
          >
            {t("action.forceQuit")}
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              void ipc.quitApp(app.key, false);
              clearQuit();
            }}
          >
            {t("action.quitApp")}
          </Button>
        </DialogActions>
      </DialogContent>
    </Dialog>
  );
}
