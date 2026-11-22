import { useMemo, useState, type CSSProperties } from "react";
import { appLabel } from "../lib/apps";
import { formatPercent } from "../lib/format";
import { treemap, type TreemapBox, type TreemapData } from "../lib/treemap";
import { useStore } from "../lib/store";
import type { AppInfo } from "../lib/types";
import { Button } from "./ui/button";

/** Deeper with the app's share, so the biggest boxes read first; capped for text contrast. */
const tint = (box: TreemapBox) =>
  box.kind === "other" ? 12 : Math.min(36, 20 + box.share * 80);

/**
 * The apps a drilled level was opened on. Keys rather than app objects, so the level follows the
 * live sample when one of them grows or closes.
 */
interface Level {
  keys: readonly string[];
}

export interface AppTreemapProps {
  apps: readonly AppInfo[];
  valueOf: (app: AppInfo) => number;
  label: string;
  tone: string;
  format: (value: number) => string;
  caption: (mapped: number) => string;
  levelCaption: (count: number, mapped: number) => string;
}

export function AppTreemap({ apps, valueOf, label, tone, format, caption, levelCaption }: AppTreemapProps) {
  const { units, t, openDetail } = useStore();
  const [trail, setTrail] = useState<Level[]>([]);
  // The pointer holds the layout still: a box that moves while it is being aimed at is a box
  // that gets the wrong app's detail sheet.
  const [held, setHeld] = useState<TreemapData | null>(null);
  // Filtered against the live apps rather than the apps the level was opened on, so a level
  // keeps meaning the same programs while their sizes change under it.
  const scoped = useMemo(() => {
    const keys = trail[trail.length - 1]?.keys;
    if (!keys) return apps;
    const scope = new Set(keys);
    return apps.filter((app) => scope.has(app.key));
  }, [apps, trail]);
  const live = useMemo(() => treemap(scoped, valueOf), [scoped, valueOf]);
  const map = held ?? live;
  const level = trail[trail.length - 1] ?? null;
  const appCount = map.boxes.reduce((count, box) => count + (box.kind === "app" ? 1 : box.apps.length), 0);
  const enter = (box: Extract<TreemapBox, { kind: "other" }>) => {
    setTrail((current) => [...current, { keys: box.apps.map((app) => app.key) }]);
    setHeld(null);
  };
  const back = () => {
    setTrail((current) => current.slice(0, -1));
    setHeld(null);
  };

  if (map.boxes.length === 0 && !level) return <p className="empty">{t("treemap.empty")}</p>;

  return (
    <div
      className="app-map-card"
      onPointerEnter={() => setHeld(live)}
      onPointerLeave={() => setHeld(null)}
    >
      {level && (
        <div className="app-map-trail">
          <Button variant="chip" onClick={back}>{t("treemap.back")}</Button>
          <span className="faint">{t("treemap.other")}</span>
        </div>
      )}
      <div
        className="app-map"
        role="group"
        aria-label={label}
        style={{ ["--map-tone" as string]: tone } as CSSProperties}
      >
        {map.boxes.length === 0 && <p className="empty">{t("treemap.empty")}</p>}
        {map.boxes.map((box) => {
          const rect = box.rect;
          const style = {
            left: `${rect.x * 100}%`,
            top: `${rect.y * 100}%`,
            width: `${rect.w * 100}%`,
            height: `${rect.h * 100}%`,
            ["--tint" as string]: `${tint(box)}%`,
          } as CSSProperties;
          const name = box.kind === "app" ? appLabel(box.app, t) : t("treemap.other");
          const size =
            box.kind === "app"
              ? format(box.value)
              : t("treemap.otherHint", { count: box.apps.length, value: format(box.value) });
          const title =
            box.kind === "app"
              ? `${name} · ${size} · ${formatPercent(box.share * 100, units.locale)}`
              : `${name} · ${size} · ${t("treemap.enter")}`;
          // Two lines need about 45px: 12px of padding, 16px for the name and 15px for the
          // size. A shorter box gets the name alone, and a tiny one is a plain rectangle.
          const showName = rect.w > 0.12 && rect.h > 0.11;
          const showSize = showName && rect.h > 0.18;
          const showEnter = box.kind === "other" && showSize && rect.h > 0.24;
          const content = showName && (
            <>
              <span className="map-box-name">{name}</span>
              {showSize && <span className="map-box-size num">{size}</span>}
              {showEnter && <span className="map-box-enter">{t("treemap.enter")}</span>}
            </>
          );
          return box.kind === "app" ? (
            <button
              key={box.app.key}
              type="button"
              className="map-box treemap-box"
              style={style}
              title={title}
              aria-label={title}
              onClick={() =>
                openDetail(box.app.key, {
                  key: box.app.key,
                  name: box.app.name,
                  exePath: box.app.exePath,
                  kind: box.app.kind,
                })
              }
            >
              {content}
            </button>
          ) : (
            <div
              key="other"
              className="map-box treemap-box other"
              style={style}
              title={title}
              aria-label={title}
              role="button"
              tabIndex={0}
              onDoubleClick={() => enter(box)}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                enter(box);
              }}
            >
              {content}
            </div>
          );
        })}
      </div>
      <p className="hint faint">
        {level ? levelCaption(appCount, map.mappedValue) : caption(map.mappedValue)}
      </p>
    </div>
  );
}
