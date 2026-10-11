import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
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
import { useStore, type QuitRequest } from "../lib/store";
import { quitMessageKey } from "../lib/quit";
import { formatBytes, formatPercent, formatRate, cpuDisplay } from "../lib/format";
import { holdCeiling, niceCeiling, type HeldCeiling } from "../lib/spark";
import { seriesPaths, type BuiltSeries } from "../lib/series";
import {
  appLabel,
  appValue,
  canQuit,
  holdOrder,
  orderApps,
  visibleWindowTitles,
  type AppMetric,
} from "../lib/apps";
import { usePins } from "../lib/pins";
import { AppMenu, isMenuKey, menuTarget, type AppMenuTarget } from "./AppMenu";
import type { UnitPrefs } from "../lib/format";
import type { Translate } from "../i18n";
import type { AppInfo, Availability } from "../lib/types";
import { UNREADABLE_GROUP } from "../lib/types";

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

/**
 * A chart ceiling that rises at once and falls slowly (see `holdCeiling`). The rise is applied
 * in the same render that brings the peak, not one effect later: for that one frame the spike
 * used to be drawn flattened against the old ceiling.
 */
export function useHeldCeiling(observed: number, holdSamples = 6): number {
  const held = useRef<HeldCeiling | null>(null);
  if (held.current === null) held.current = { ceiling: niceCeiling(observed), below: 0 };
  const [ceiling, setCeiling] = useState(held.current.ceiling);

  useEffect(() => {
    const next = holdCeiling(held.current ?? { ceiling: niceCeiling(observed), below: 0 }, observed, holdSamples);
    held.current = next;
    setCeiling(next.ceiling);
  }, [observed, holdSamples]);

  return Math.max(ceiling, niceCeiling(observed));
}

/**
 * The live chart: the last few minutes at one sample a second, so the samples are evenly
 * spaced and the index is the time axis. History has gaps and is drawn by HistoryPlot instead.
 *
 * `ceiling` fixes the top of the scale, which is right for a percentage: fitted to the data, an
 * idle machine's 3% filled the card. Without it the scale follows the data with a held ceiling.
 */
export function Chart({
  values,
  ceiling,
  color,
  variant = "area",
  height = 56,
}: {
  values: number[];
  ceiling?: number;
  color: string;
  variant?: "area" | "line" | "bars";
  height?: number;
}) {
  const width = 240;
  const held = useHeldCeiling(values.length > 0 ? Math.max(...values) : 0);
  const top = ceiling ?? held;
  // No samples yet draws an empty chart, not a line along zero that nobody measured.
  if (values.length === 0) {
    return variant === "bars" ? (
      <div className="bars" style={{ height }} />
    ) : (
      <svg className="chart" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" />
    );
  }
  const normalized = values.map((v) => Math.min(1, Math.max(0, v / top)));

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

/**
 * A stretch of history on its real time axis (see buildSeries): each run of buckets is its own
 * subpath, so a stretch with no record is blank rather than joined up. The lighter band behind
 * the line is the highest value each bucket reached, which is what an average hides.
 */
export const HistoryPlot = memo(function HistoryPlot({
  built,
  ceiling,
  color,
  height,
  label,
}: {
  built: BuiltSeries;
  ceiling: number;
  color: string;
  height: number;
  label?: string;
}) {
  const width = 240;
  const paths = useMemo(() => seriesPaths(built, width, height, ceiling), [built, height, ceiling]);
  return (
    <svg
      className="chart history-plot"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={label}
      style={{ height }}
    >
      {paths.envelope && <path d={paths.envelope} fill={color} opacity={0.1} />}
      {paths.area && <path d={paths.area} fill={color} opacity={0.18} />}
      {paths.line && (
        <path
          d={paths.line}
          fill="none"
          stroke={color}
          strokeWidth={1.5}
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
      )}
    </svg>
  );
});

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

export type { AppMetric } from "../lib/apps";
export { appValue } from "../lib/apps";

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

interface RowProps {
  app: AppInfo;
  index: number;
  offset: number;
  label: string;
  value: string;
  ratio: number;
  pinned: boolean;
  t: Translate;
  measure: (node: Element | null) => void;
  onMenu: (app: AppInfo, label: string, element: HTMLElement, event?: React.MouseEvent<HTMLElement>) => void;
  onQuit: (app: AppInfo) => void;
}

/**
 * One application. The row itself takes focus, so the keyboard reaches the same menu the
 * pointer does (ContextMenu key or Shift+F10); the × stays a pointer shortcut out of the tab
 * order, since the menu's Quit… covers it.
 */
function AppRow({ app, index, offset, label, value, ratio, pinned, t, measure, onMenu, onQuit }: RowProps) {
  // The synthetic row is a count of processes that need administrative rights to open, not an
  // application: it has no figures, and it must never offer to close anything.
  const unreadable = app.key === UNREADABLE_GROUP;
  const hint = unreadable ? t("list.unreadableHint") : visibleWindowTitles(app).join("\u000a") || app.exePath || label;
  return (
    <Hint className="max-w-[420px] whitespace-pre-line text-left" label={hint}>
      <div
        data-index={index}
        ref={measure}
        tabIndex={0}
        className={`row ${pinned ? "pinned" : ""}`}
        style={{ position: "absolute", top: 0, left: 0, right: 0, transform: `translateY(${offset}px)` }}
        onContextMenu={(event) => {
          event.preventDefault();
          onMenu(app, label, event.currentTarget, event);
        }}
        onKeyDown={(event) => {
          if (!isMenuKey(event)) return;
          event.preventDefault();
          onMenu(app, label, event.currentTarget);
        }}
      >
        <AppIcon name={label} seed={app.key} exePath={app.exePath || undefined} />
        <div className="row-main">
          <span className="row-name">{label}</span>
          <span className={`row-sub ${KIND_TONE[app.kind]}`}>
            {t(`kind.${app.kind}`)}
            {app.processCount > 1 ? ` · ${app.processCount} ${t("label.processes").toLowerCase()}` : ""}
            {app.listeningPorts.length > 0 ? ` · ${app.listeningPorts.slice(0, 4).join(", ")}` : ""}
          </span>
        </div>
        <div className="row-meter">
          <Meter ratio={unreadable ? 0 : ratio} color="var(--accent)" />
        </div>
        <span className="row-value num">{unreadable ? "—" : value}</span>
        {canQuit(app) && (
          <button className="row-quit" tabIndex={-1} onClick={() => onQuit(app)} aria-label={t("action.quitApp")}>
            ×
          </button>
        )}
      </div>
    </Hint>
  );
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
  const [pins, togglePin] = usePins();
  const [query, setQuery] = useState(seedQuery ?? "");
  const [held, setHeld] = useState<string[] | null>(null);
  const [menu, setMenu] = useState<AppMenuTarget | null>(null);
  const pointerInside = useRef(false);
  useEffect(() => {
    if (seedQuery !== undefined) setQuery(seedQuery);
  }, [seedQuery]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return apps;
    return apps.filter(
      (a) =>
        appLabel(a, t).toLowerCase().includes(needle) ||
        a.name.toLowerCase().includes(needle) ||
        a.exePath.toLowerCase().includes(needle) ||
        a.pids.some((p) => String(p) === needle),
    );
  }, [apps, query, t]);

  const ordered = useMemo(() => orderApps(filtered, pins, metric), [filtered, pins, metric]);
  const latest = useRef(ordered);
  latest.current = ordered;

  // The cap comes after the hold, so a held list keeps the same rows, not just the same order.
  const rows = useMemo(() => {
    const shown = holdOrder(ordered, held);
    return cap ? shown.slice(0, cap) : shown;
  }, [ordered, held, cap]);

  const hold = useCallback(() => setHeld((current) => current ?? latest.current.map((a) => a.key)), []);

  // A pin, a new sort or a new search is the user's own doing: show its result at once, and
  // keep holding that order if the list was being held.
  const pinsKey = pins.join("\u000a");
  useEffect(() => {
    setHeld((current) => (current ? latest.current.map((a) => a.key) : null));
  }, [pinsKey, metric, query]);

  useEffect(() => {
    if (!held) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) setHeld(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [held]);

  const openMenu = useCallback(
    (app: AppInfo, label: string, element: HTMLElement, event?: React.MouseEvent<HTMLElement>) => {
      hold();
      setMenu(menuTarget(app, label, element, event));
    },
    [hold],
  );
  const closeMenu = useCallback(() => {
    setMenu(null);
    if (!pointerInside.current) setHeld(null);
  }, []);

  const peak = Math.max(1, ...rows.map((a) => appValue(a, metric)));

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
    count: rows.length,
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
        data-count={rows.length}
        data-held={held ? "" : undefined}
        style={rows.length > 0 ? { position: "relative", height: virtualizer.getTotalSize() } : undefined}
        onPointerEnter={() => {
          pointerInside.current = true;
          hold();
        }}
        onPointerLeave={() => {
          pointerInside.current = false;
          if (!menu) setHeld(null);
        }}
      >
        {virtualizer.getVirtualItems().map((item) => {
          const app = rows[item.index];
          if (!app) return null;
          return (
            <AppRow
              key={app.key}
              app={app}
              index={item.index}
              offset={item.start - scrollMargin}
              label={appLabel(app, t)}
              value={format(app)}
              ratio={appValue(app, metric) / peak}
              pinned={pins.includes(app.key)}
              t={t}
              measure={virtualizer.measureElement}
              onMenu={openMenu}
              onQuit={requestQuit}
            />
          );
        })}
        {rows.length === 0 && <p className="empty faint">{t("state.noData")}</p>}
      </div>
      {menu && (
        <AppMenu target={menu} pinned={pins.includes(menu.app.key)} onTogglePin={togglePin} onClose={closeMenu} />
      )}
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
  const { pendingQuit, clearQuit, requestQuit, pushToast, t } = useStore();
  const lastAskedAbout = useRef<QuitRequest | null>(null);
  if (pendingQuit) lastAskedAbout.current = pendingQuit;
  const request = pendingQuit ?? lastAskedAbout.current;
  if (!request) return null;
  const { app, preferForce } = request;
  const name = appLabel(app, t);
  const title = t(preferForce ? "action.confirmForceQuit" : "action.confirmQuit", { name });

  // The answer used to be discarded, so an app with no visible window ignored Quit in silence.
  // It now comes back as a notice, and "no window" offers the one thing that can still work.
  const run = (force: boolean) => {
    clearQuit();
    void ipc.quitApp(app.key, force).then((result) => {
      const outcome = quitMessageKey(result);
      pushToast({
        tone: outcome.tone,
        text: t(outcome.key, { name, count: outcome.count }),
        action: outcome.offerForce
          ? { label: t("action.forceQuit"), run: () => requestQuit(app, true) }
          : undefined,
      });
    });
  };

  return (
    <Dialog open={Boolean(pendingQuit)} onOpenChange={(next) => !next && clearQuit()}>
      <DialogContent label={title}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className="text-muted-foreground">
            {preferForce
              ? t("action.confirmForceQuitBody", { count: app.processCount })
              : t("action.confirmQuitBody", { count: app.processCount })}
          </DialogDescription>
          <DialogDescription className="mt-1.5 truncate text-[11.5px] text-ink-3">
            {visibleWindowTitles(app).slice(0, 3).join(" · ")}
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
          {/* After "no window to close", force quit is the answer being offered, so it is the
              filled button and Quit steps back - but focus still starts on Cancel. */}
          <Button
            variant="danger"
            className={
              preferForce ? "border-transparent bg-destructive text-destructive-foreground hover:bg-destructive" : undefined
            }
            onClick={() => run(true)}
          >
            {t("action.forceQuit")}
          </Button>
          <Button variant={preferForce ? "chip" : "primary"} onClick={() => run(false)}>
            {t("action.quitApp")}
          </Button>
        </DialogActions>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Outcomes that need saying but not answering: they sit in a corner, go away on their own,
 * and are read out politely by a screen reader rather than interrupting it.
 */
export function Toaster() {
  const { toasts, dismissToast, t } = useStore();
  return (
    <div className="toaster" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast ${toast.tone}`}>
          <span className="toast-text">{toast.text}</span>
          {toast.action && (
            <Button
              variant="chip"
              className="border-input"
              onClick={() => {
                dismissToast(toast.id);
                toast.action?.run();
              }}
            >
              {toast.action.label}
            </Button>
          )}
          <button className="toast-close" aria-label={t("action.dismiss")} onClick={() => dismissToast(toast.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
