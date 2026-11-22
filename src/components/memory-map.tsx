import { useMemo, useState, type CSSProperties } from "react";
import { appLabel } from "../lib/apps";
import { formatBytes, formatPercent } from "../lib/format";
import { memoryMap, type MemoryBox, type MemoryMapData } from "../lib/memory-map";
import { useStore } from "../lib/store";
import type { AppInfo } from "../lib/types";

/** Deeper with the app's share, so the biggest boxes read first; capped for text contrast. */
const tint = (box: MemoryBox) =>
  box.kind === "other" ? 12 : Math.min(36, 20 + box.share * 80);

export function MemoryMap({ apps, inUseBytes }: { apps: readonly AppInfo[]; inUseBytes: number }) {
  const { units, t, openDetail } = useStore();
  // The pointer holds the layout still: a box that moves while it is being aimed at is a box
  // that gets the wrong app's detail sheet.
  const [held, setHeld] = useState<MemoryMapData | null>(null);
  const live = useMemo(() => memoryMap(apps), [apps]);
  const map = held ?? live;

  if (map.boxes.length === 0) return <p className="empty">{t("memoryMap.empty")}</p>;

  return (
    <div
      className="memory-map-card"
      onPointerEnter={() => setHeld(live)}
      onPointerLeave={() => setHeld(null)}
    >
      <div className="memory-map" role="group" aria-label={t("memoryMap.title")}>
        {map.boxes.map((box) => {
          const rect = box.rect;
          const style = {
            left: `${rect.x * 100}%`,
            top: `${rect.y * 100}%`,
            width: `${rect.w * 100}%`,
            height: `${rect.h * 100}%`,
            ["--tint" as string]: `${tint(box)}%`,
          } as CSSProperties;
          const label = box.kind === "app" ? appLabel(box.app, t) : t("memoryMap.other");
          const size =
            box.kind === "app"
              ? formatBytes(box.bytes, units)
              : t("memoryMap.otherHint", { count: box.count, bytes: formatBytes(box.bytes, units) });
          const title =
            box.kind === "app"
              ? `${label} · ${size} · ${formatPercent(box.share * 100, units.locale)}`
              : `${label} · ${size}`;
          // Two lines need about 45px: 12px of padding, 16px for the name and 15px for the
          // size. A shorter box gets the name alone, and a tiny one is a plain rectangle.
          const showName = rect.w > 0.12 && rect.h > 0.11;
          const showSize = showName && rect.h > 0.18;
          const content = showName && (
            <>
              <span className="map-box-name">{label}</span>
              {showSize && <span className="map-box-size num">{size}</span>}
            </>
          );
          return box.kind === "app" ? (
            <button
              key={box.app.key}
              type="button"
              className="map-box mem-box"
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
            <div key="other" className="map-box mem-box other" style={style} title={title}>
              {content}
            </div>
          );
        })}
      </div>
      <p className="hint faint">
        {t("memoryMap.help", {
          mapped: formatBytes(map.mappedBytes, units),
          inUse: formatBytes(inUseBytes, units),
        })}
      </p>
    </div>
  );
}
