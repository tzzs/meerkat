import { useEffect, useMemo, useRef, useState } from "react";
import { cpuDisplay, formatBytes, formatPercent, formatTemp } from "../lib/format";
import { ipc } from "../lib/ipc";
import { AppIcon, QuitDialog } from "../components/ui";
import { useStore } from "../lib/store";
import { headline, seriesFor, type MetricKind, type Snapshot, type TrayReadout } from "../lib/types";

const HUES: Record<MetricKind, string> = {
  cpu: "#5ecb8f",
  memory: "#64a9ff",
  disk: "#f0b35f",
  network: "#4fd2da",
  gpu: "#ea7ac6",
  battery: "#a3d75f",
  temperature: "#ff8b5e",
};

export function PanelApp() {
  const { snapshot, settings, t, units, connected, requestQuit } = useStore();
  const [pinned, setPinned] = useState(false);
  const [tab, setTab] = useState<MetricKind | "top">("top");
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void ipc.hidePanel();
      if (event.key >= "1" && event.key <= "9") {
        const order: (MetricKind | "top")[] = ["top", "cpu", "memory", "disk", "network", "gpu", "battery", "temperature"];
        const target = order[Number(event.key) - 1];
        if (target) setTab(target);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const measure = () => {
      const node = bodyRef.current;
      if (!node) return;
      const width = Math.max(320, node.scrollWidth);
      const height = Math.min(760, Math.max(220, node.scrollHeight));
      void ipc.setPanelSize(width, height);
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (bodyRef.current) observer.observe(bodyRef.current);
    return () => observer.disconnect();
  }, [snapshot, settings.panel.layout, tab]);

  useEffect(() => {
    if (!snapshot) return;
    void renderTrayIcons(settings.tray, snapshot, units);
  }, [snapshot, settings.tray, units]);

  const apps = useMemo(() => {
    const list = snapshot?.apps ?? [];
    return [...list].sort((a, b) => b.memoryBytes - a.memoryBytes);
  }, [snapshot]);

  const tiles = settings.panel.tiles.filter((x) => x.visible).map((x) => x.id as MetricKind);
  const shown = tab === "top" ? [] : [tab];

  return (
    <div className="panel" ref={bodyRef}>
      <header className="panel-head">
        <span className="brand">
          <span className="brand-dot" />
          Pulse
        </span>
        <span className="panel-machine">
          {snapshot
            ? `${snapshot.machine.chipName.split(" ").slice(0, 3).join(" ")} · ${formatBytes(snapshot.memory.inUseBytes, units)}`
            : t("state.noData")}
        </span>
        <button
          className={`icon-btn ${pinned ? "on" : ""}`}
          title={t("panel.pin")}
          onClick={() => {
            const next = !pinned;
            setPinned(next);
            ipc.setPanelPinned(next);
          }}
        >
          ◎
        </button>
        <button className="icon-btn" title={t("panel.openWindow")} onClick={() => void ipc.showWindow()}>
          ⤢
        </button>
      </header>

      <div className={`panel-metrics ${settings.panel.layout}`}>
        {(shown.length ? shown : tiles).map((metric) =>
          snapshot ? (
            settings.panel.layout === "list" ? (
              <ListRow key={metric} metric={metric} snapshot={snapshot} t={t} units={units} />
            ) : (
              <Tile key={metric} metric={metric} snapshot={snapshot} t={t} units={units} />
            )
          ) : null,
        )}
      </div>

      {tab === "top" && (
        <div className="panel-apps">
          {apps.slice(0, 6).map((app) => (
            <div className="panel-app" key={app.key} title={app.exePath}>
              <AppIcon name={app.name} seed={app.key} exePath={app.exePath || undefined} />
              <span className="panel-app-name">{app.name}</span>
              <span className="panel-app-mem num">{formatBytes(app.memoryBytes, units)}</span>
              <span className="panel-app-cpu num">{formatPercent(cpuDisplay(app.cpuPercent, units), units.locale)}</span>
              <button
                className="row-quit"
                onClick={() => {
                  requestQuit(app);
                }}
                title={t("action.quitApp")}
              >
                ×
              </button>
            </div>
          ))}
          {apps.length === 0 && <p className="empty faint">{connected ? "" : t("state.noData")}</p>}
        </div>
      )}

      <nav className="panel-tabs">
        <button className={tab === "top" ? "on" : ""} onClick={() => setTab("top")}>
          {t("tab.overview")}
        </button>
        {(["cpu", "memory", "disk", "network", "gpu", "battery"] as MetricKind[]).map((metric) => (
          <button key={metric} className={tab === metric ? "on" : ""} onClick={() => setTab(metric)}>
            {t(`metric.${metric}` as "metric.cpu")}
          </button>
        ))}
      </nav>
      <QuitDialog />
    </div>
  );
}

function Tile({
  metric,
  snapshot,
  t,
  units,
}: {
  metric: MetricKind;
  snapshot: Snapshot;
  t: ReturnType<typeof useStore>["t"];
  units: ReturnType<typeof useStore>["units"];
}) {
  const value = textFor(metric, snapshot, units);
  const series = seriesFor(metric, snapshot);
  return (
    <div className="tile" style={{ ["--hue" as string]: HUES[metric] }}>
      <span className="tile-label">{t(`metric.${metric}` as "metric.cpu")}</span>
      <span className="tile-value num">{value}</span>
      <svg className="tile-spark" viewBox="0 0 100 22" preserveAspectRatio="none">
        <polyline
          points={spark(series, 100, 22)}
          fill="none"
          stroke={HUES[metric]}
          strokeWidth={1.2}
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </div>
  );
}

function ListRow({
  metric,
  snapshot,
  t,
  units,
}: {
  metric: MetricKind;
  snapshot: Snapshot;
  t: ReturnType<typeof useStore>["t"];
  units: ReturnType<typeof useStore>["units"];
}) {
  return (
    <div className="list-row" style={{ ["--hue" as string]: HUES[metric] }}>
      <span className="list-label">{t(`metric.${metric}` as "metric.cpu")}</span>
      <span className="list-value num">{textFor(metric, snapshot, units)}</span>
      <svg className="list-spark" viewBox="0 0 100 14" preserveAspectRatio="none">
        <polyline points={spark(seriesFor(metric, snapshot), 100, 14)} fill="none" stroke={HUES[metric]} strokeWidth={1} vectorEffect="non-scaling-stroke" />
      </svg>
    </div>
  );
}

function spark(values: number[], width: number, height: number): string {
  if (values.length < 2) return `0,${height} ${width},${height}`;
  const max = Math.max(...values, 1);
  const step = width / (values.length - 1);
  return values
    .slice(-60)
    .map((v, i) => `${(i * step).toFixed(1)},${(height - (v / max) * height).toFixed(1)}`)
    .join(" ");
}

function textFor(metric: MetricKind, snapshot: Snapshot, units: ReturnType<typeof useStore>["units"]): string {
  switch (metric) {
    case "cpu":
      return `${Math.round(headline("cpu", snapshot))}%`;
    case "memory":
      return `${formatPercent(headline("memory", snapshot), units.locale)} · ${formatBytes(snapshot.memory.inUseBytes, units)}`;
    case "disk":
      return `↓${shortRate(snapshot.disk.readBytesPerSecond, units)} ↑${shortRate(snapshot.disk.writeBytesPerSecond, units)}`;
    case "network":
      return `↓${shortRate(snapshot.network.downloadBytesPerSecond, units)} ↑${shortRate(snapshot.network.uploadBytesPerSecond, units)}`;
    case "gpu":
      return `${Math.round(headline("gpu", snapshot))}%`;
    case "battery":
      return snapshot.battery.hasBattery ? `${Math.round(headline("battery", snapshot))}%` : "—";
    case "temperature":
      return snapshot.temperature.hottest ? formatTemp(snapshot.temperature.hottest, units) : "—";
  }
}

/**
 * Tray readouts are drawn here and pushed to Rust as PNG bytes, so the numbers in the
 * taskbar corner use the same face as the panel.
 */
async function renderTrayIcons(
  tray: TrayReadout[],
  snapshot: Snapshot,
  units: ReturnType<typeof useStore>["units"],
): Promise<void> {
  const ids = await ipc.trayIds();
  const byMetric = new Map<string, string>();
  for (const readout of tray) {
    if (!readout.enabled) continue;
    byMetric.set(readout.metric, readout.id);
  }

  for (const id of ids) {
    const metric = id.replace(/^tray-/, "");
    const readout = tray.find((r) => r.id === metric);
    if (!readout || !readout.enabled) continue;
    const png = drawReadout(readout, snapshot, units);
    if (png) await ipc.setTrayBitmap(id, png);
  }
}

function drawReadout(
  readout: TrayReadout,
  snapshot: Snapshot,
  units: ReturnType<typeof useStore>["units"],
): number[] | null {
  const height = 34;
  const scale = 2;
  const label = compactText(readout, snapshot, units);
  const canvas = document.createElement("canvas");
  const width = Math.min(160, Math.max(38, label.length * 9 + 14));
  canvas.width = width * scale;
  canvas.height = height * scale;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.scale(scale, scale);
  ctx.clearRect(0, 0, width, height);

  const hue = HUES[readout.metric];
  const drawGraph = readout.style === "graph" || readout.style === "both";
  const drawValue = readout.style === "value" || readout.style === "both";

  if (drawValue) {
    ctx.font = "600 12px 'Segoe UI Variable Text', 'Segoe UI', sans-serif";
    ctx.textBaseline = "top";
    ctx.lineWidth = 2.6;
    ctx.strokeStyle = "rgba(8,10,14,0.75)";
    ctx.fillStyle = "#f2f4f8";
    ctx.strokeText(label, 5, drawGraph ? 3 : 9);
    ctx.fillText(label, 5, drawGraph ? 3 : 9);
  }
  if (drawGraph) {
    const values = seriesFor(readout.metric, snapshot).slice(-24);
    const baseY = height - 5;
    const top = height - (drawValue ? 20 : 6);
    const max = Math.max(...values, 1);
    ctx.beginPath();
    values.forEach((value, index) => {
      const x = 5 + (index / Math.max(1, values.length - 1)) * (width - 10);
      const y = baseY - (value / max) * (baseY - top);
      if (index === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.strokeStyle = hue;
    ctx.lineWidth = 1.6;
    ctx.stroke();
  }

  const data = canvas.toDataURL("image/png");
  if (!ipc.enabled) {
    // Lets the dev browser show exactly what the taskbar corner received.
    (window as unknown as Record<string, unknown>).__trayPreview = data;
  }
  const base64 = data.slice(data.indexOf(",") + 1);
  const bytes = atobToBytes(base64);
  return Array.from(bytes);
}

function atobToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function compactText(readout: TrayReadout, snapshot: Snapshot, units: ReturnType<typeof useStore>["units"]): string {
  switch (readout.metric) {
    case "cpu":
      return `${Math.round(headline("cpu", snapshot))}%`;
    case "memory": {
      const ratio = readout.invert
        ? (snapshot.memory.availableBytes / Math.max(1, snapshot.memory.totalBytes)) * 100
        : (snapshot.memory.inUseBytes / Math.max(1, snapshot.memory.totalBytes)) * 100;
      return `${Math.round(ratio)}%`;
    }
    case "disk":
      return `${shortRate(snapshot.disk.writeBytesPerSecond, units)}`;
    case "network":
      return `${shortRate(snapshot.network.downloadBytesPerSecond, units)}`;
    case "gpu":
      return `${Math.round(headline("gpu", snapshot))}%`;
    case "battery":
      return snapshot.battery.hasBattery ? `${Math.round(headline("battery", snapshot))}%` : "—";
    case "temperature":
      return snapshot.temperature.hottest ? `${Math.round(snapshot.temperature.hottest)}°` : "—";
  }
}

function shortRate(bytesPerSecond: number, units: ReturnType<typeof useStore>["units"]): string {
  const value = units.network === "bits" ? bytesPerSecond * 8 : bytesPerSecond;
  const step = units.network === "bits" ? 1000 : 1024;
  const labels = units.network === "bits" ? ["b", "K", "M", "G"] : ["B", "K", "M", "G"];
  let index = 0;
  let scaled = value;
  while (scaled >= step && index < labels.length - 1) {
    scaled /= step;
    index += 1;
  }
  return `${scaled >= 100 ? Math.round(scaled) : scaled.toFixed(1)}${labels[index]}`;
}
