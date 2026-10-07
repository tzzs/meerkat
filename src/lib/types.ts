/**
 * The wire contract between the Rust sampler and the UI.
 * Rust mirrors these with `#[serde(rename_all = "camelCase")]` in `src-tauri/src/models.rs`;
 * that file is the authority — change both together.
 */

export type MetricKind = "cpu" | "memory" | "disk" | "network" | "gpu" | "battery" | "temperature";

export type Availability = "ok" | "partial" | "unavailable" | "needsElevation";

export interface MachineInfo {
  hostname: string;
  osVersion: string;
  chipName: string;
  physicalCores: number | null;
  logicalCores: number;
  totalMemoryBytes: number;
  uptimeSeconds: number;
}

export interface CpuMetric {
  totalPercent: number;
  userPercent: number;
  privilegedPercent: number;
  corePercents: number[];
  processorQueueLength: number | null;
  history: number[];
}

export interface MemoryMetric {
  totalBytes: number;
  inUseBytes: number;
  availableBytes: number;
  cachedBytes: number;
  committedBytes: number;
  commitLimitBytes: number;
  pagefileTotalBytes: number;
  pagefileUsedBytes: number;
  pagedPoolBytes: number;
  nonpagedPoolBytes: number;
  pressurePercent: number;
  history: number[];
}

export interface VolumeInfo {
  id: string;
  name: string;
  mountPoint: string;
  fileSystem: string;
  totalBytes: number;
  usedBytes: number;
  freeBytes: number;
  readBytesPerSecond: number;
  writeBytesPerSecond: number;
  isRemovable: boolean;
  isNetwork: boolean;
  isSolidState: boolean;
}

export interface DiskMetric {
  readBytesPerSecond: number;
  writeBytesPerSecond: number;
  readBytesToday: number;
  writeBytesToday: number;
  volumes: VolumeInfo[];
  historyRead: number[];
  historyWrite: number[];
  availability: Availability;
}

export interface AdapterInfo {
  name: string;
  isUp: boolean;
  linkSpeedBitsPerSecond: number | null;
  downloadBytesPerSecond: number;
  uploadBytesPerSecond: number;
  isLoopback: boolean;
}

export interface NetworkMetric {
  downloadBytesPerSecond: number;
  uploadBytesPerSecond: number;
  downloadBytesToday: number;
  uploadBytesToday: number;
  adapters: AdapterInfo[];
  connections: number;
  historyDownload: number[];
  historyUpload: number[];
}

export interface GpuAdapter {
  index: number;
  name: string;
  dedicatedTotalBytes: number | null;
  dedicatedUsedBytes: number | null;
  sharedUsedBytes: number | null;
  temperatureC: number | null;
  fanPercent: number | null;
  powerWatts: number | null;
  utilizationPercent: number | null;
}

export interface GpuProcess {
  pid: number;
  percent: number;
}

export interface GpuMetric {
  totalPercent: number | null;
  adapters: GpuAdapter[];
  perProcess: GpuProcess[];
  availability: Availability;
  history: number[];
}

export interface BatteryMetric {
  hasBattery: boolean;
  onAcPower: boolean;
  chargePercent: number | null;
  isCharging: boolean;
  isCritical: boolean;
  remainingSeconds: number | null;
  powerWatts: number | null;
  designCapacityWh: number | null;
  fullChargeCapacityWh: number | null;
  healthPercent: number | null;
  cycleCount: number | null;
  source: Availability;
  history: number[];
}

export interface SensorReading {
  id: string;
  label: string;
  celsius: number;
}

export interface TemperatureMetric {
  cpuCelsius: number | null;
  gpuCelsius: number | null;
  hottest: number | null;
  sensors: SensorReading[];
  availability: Availability;
  history: number[];
}

export type AppKind = "app" | "background" | "service" | "system" | "packaged";

export interface WindowInfo {
  title: string;
  pid: number;
  visible: boolean;
  hwnd: number;
}

export interface AppInfo {
  /** Stable identity: the main executable path, lowercased. */
  key: string;
  name: string;
  exePath: string;
  kind: AppKind;
  mainPid: number;
  processCount: number;
  cpuPercent: number;
  memoryBytes: number;
  readBytesPerSecond: number;
  writeBytesPerSecond: number;
  readBytesTotal: number;
  writeBytesTotal: number;
  connections: number;
  listeningPorts: number[];
  oldestStartedAtMs: number | null;
  windows: WindowInfo[];
  pids: number[];
  unmeasuredProcesses: number;
}

export interface Snapshot {
  takenAtMs: number;
  machine: MachineInfo;
  cpu: CpuMetric;
  memory: MemoryMetric;
  disk: DiskMetric;
  network: NetworkMetric;
  gpu: GpuMetric;
  battery: BatteryMetric;
  temperature: TemperatureMetric;
  apps: AppInfo[];
  processCount: number;
  samplerCostMicroseconds: number;
}

export interface HistoryPoint {
  atMs: number;
  value: number;
  secondary: number | null;
}

export interface HistorySeries {
  metric: string;
  bucketSeconds: number;
  rangeSeconds: number;
  points: HistoryPoint[];
}

export interface RankedApp {
  key: string;
  name: string;
  value: number;
}

export interface RankingRow {
  hourStartMs: number;
  metric: string;
  apps: RankedApp[];
}

export type InsightKind = "highCpu" | "growingMemory" | "heavyDisk" | "heavyNetwork" | "heat";

export interface Insight {
  id: string;
  atMs: number;
  kind: InsightKind;
  appKey: string;
  appName: string;
  value: number;
  sustainedSeconds: number;
  repeatCount: number;
  dayKey: string;
  resolved: boolean;
}

export type TrayStyle = "value" | "graph" | "both";
export type PanelLayout = "tiles" | "list";

export interface TrayReadout {
  id: string;
  metric: MetricKind;
  style: TrayStyle;
  showIcon: boolean;
  showCaption: boolean;
  invert: boolean;
  enabled: boolean;
}

export interface SectionConfig {
  id: string;
  visible: boolean;
}

export interface TabConfig {
  id: string;
  visible: boolean;
  sections: SectionConfig[];
}

export interface AlertThresholds {
  enabled: boolean;
  cpuPercent: number;
  memoryGrowBytes: number;
  diskBytesPerSecond: number;
  networkBytesPerSecond: number;
  sustainedSeconds: number;
}

export interface Settings {
  version: number;
  language: "auto" | "en" | "zhHans";
  temperatureUnit: "celsius" | "fahrenheit";
  networkUnit: "bytes" | "bits";
  storageUnit: "binary" | "decimal";
  cpuScale: "perCore" | "perMachine";
  theme: "auto" | "light" | "dark";
  showSystemAppsInLists: boolean;
  openWindowOn: string;
  startAtLogin: boolean;
  showInTaskbar: boolean;
  tray: TrayReadout[];
  panel: { layout: PanelLayout; tiles: SectionConfig[] };
  window: { tabs: TabConfig[] };
  alerts: AlertThresholds;
}

export const METRIC_ORDER: MetricKind[] = [
  "cpu",
  "memory",
  "disk",
  "network",
  "gpu",
  "battery",
  "temperature",
];

export const TAB_ORDER = ["overview", "cpu", "memory", "disk", "network", "gpu", "battery"] as const;
export type TabId = (typeof TAB_ORDER)[number];

/** Headline number for a metric, used by tray readouts and panel tiles alike. */
export function headline(metric: MetricKind, s: Snapshot): number {
  switch (metric) {
    case "cpu":
      return s.cpu.totalPercent;
    case "memory":
      return s.memory.totalBytes
        ? (s.memory.inUseBytes / s.memory.totalBytes) * 100
        : 0;
    case "disk":
      return s.disk.readBytesPerSecond + s.disk.writeBytesPerSecond;
    case "network":
      return s.network.downloadBytesPerSecond + s.network.uploadBytesPerSecond;
    case "gpu":
      return s.gpu.totalPercent ?? 0;
    case "battery":
      return s.battery.chargePercent ?? 0;
    case "temperature":
      return s.temperature.hottest ?? 0;
  }
}

export function seriesFor(metric: MetricKind, s: Snapshot): number[] {
  switch (metric) {
    case "cpu":
      return s.cpu.history;
    case "memory":
      return s.memory.history;
    case "disk":
      return s.disk.historyRead;
    case "network":
      return s.network.historyDownload;
    case "gpu":
      return s.gpu.history;
    case "battery":
      return s.battery.history;
    case "temperature":
      return s.temperature.history;
  }
}
