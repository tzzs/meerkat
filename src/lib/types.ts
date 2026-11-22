/**
 * The wire contract between the Rust sampler and the UI.
 * Rust mirrors these with `#[serde(rename_all = "camelCase")]` in `src-tauri/src/models.rs`;
 * that file is the authority — change both together.
 */

export type MetricKind = "cpu" | "memory" | "disk" | "network" | "gpu" | "battery" | "temperature";

export type Availability = "ok" | "partial" | "unavailable";

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

export interface ListeningPort {
  port: number;
  /** `0.0.0.0` or `::` takes connections from the network; `127.0.0.1` or `::1` only from here. */
  address: string;
  pid: number;
  appKey: string;
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
  /** Every listening TCP socket, with the app that holds it. */
  listening: ListeningPort[];
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
  /** Weekly points from Windows' battery report: what the battery holds when full, over time. */
  capacityHistory: CapacityPoint[];
}

export interface CapacityPoint {
  atMs: number;
  designWh: number;
  fullChargeWh: number;
  cycleCount: number | null;
}

/** One process of an app, for its detail view. */
export interface ProcessRow {
  pid: number;
  parentPid: number | null;
  name: string;
  exePath: string;
  commandLine: string[];
  cpuPercent: number;
  memoryBytes: number;
  privateBytes: number;
  readBytesTotal: number;
  writeBytesTotal: number;
  threads: number;
  handles: number;
  startedAtMs: number | null;
  canQuit: boolean;
}

/** Bytes moved on one local day while Meerkat was running. */
export interface DailyTotal {
  dayStartMs: number;
  readBytes: number;
  writeBytes: number;
  downloadBytes: number;
  uploadBytes: number;
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
  /** The Windows services its processes run, by the name Windows shows for them. */
  services: string[];
  /** Whether this user may end at least one of its processes; the system's own answer no. */
  canQuit: boolean;
  /** The rules Meerkat applies to it, and whether Meerkat has paused it. */
  control: AppControl;
  /** For a runtime grouped by project (node, python…): the project's folder. */
  project: string | null;
}

export type Priority = "low" | "belowNormal" | "normal" | "aboveNormal" | "high";

export interface AppControl {
  efficiency: boolean;
  priority: Priority | null;
  muteAlerts: boolean;
  paused: boolean;
}

/** What Meerkat remembers about how one app should run. */
export interface AppRule {
  key: string;
  name: string;
  efficiency: boolean;
  priority: Priority | null;
  muteAlerts: boolean;
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

/** One app over a span of its history. */
export interface AppUsage {
  key: string;
  name: string;
  exePath: string;
  kind: AppKind;
  /** How long Meerkat saw it running in the span. */
  seconds: number;
  /** CPU time used, in seconds of one core. */
  cpuSeconds: number;
  memoryAverage: number;
  memoryPeak: number;
  readBytes: number;
  writeBytes: number;
  gpuSeconds: number;
}

/** The two things the backend ranks apps by, once an hour. */
export type RankingMetric = "memory" | "cpu";

/** One app inside an hour's ranking: the figure it was ranked by, in that metric's unit. */
export interface RankedApp {
  key: string;
  name: string;
  value: number;
}

/** One hour of "the busiest apps", for one metric, from the machine-wide history store. */
export interface RankingRow {
  hourStartMs: number;
  metric: RankingMetric;
  apps: RankedApp[];
}

export type RankBy = "cpu" | "memory" | "disk" | "write" | "gpu";

export interface AppPoint {
  atMs: number;
  spanSeconds: number;
  /** Average over the point, in percent of one core. */
  cpuPercent: number;
  memoryBytes: number;
  readBytes: number;
  writeBytes: number;
}

export type SystemEventKind =
  | "appCrash"
  | "appHang"
  | "blueScreen"
  | "powerLoss"
  | "sleep"
  | "wake"
  | "boot"
  | "shutdown"
  | "driverReset"
  | "hardwareError"
  | "updateInstalled"
  | "updateFailed"
  | "diskError"
  | "serviceCrash";

export interface SystemEvent {
  atMs: number;
  kind: SystemEventKind;
  subject: string;
  detail: string;
}

/** Whether a Bluetooth device talks the modern low-energy protocol or the older classic one. */
export type BluetoothKind = "le" | "classic";

/**
 * What a device's battery can say. `percent` is the one case with a number; the rest are the
 * honest reasons there is none, and they are different reasons: no service to read, a service that
 * answered with nothing usable, and a device that never answered at all.
 */
export type BluetoothBattery =
  | { state: "percent"; percent: number }
  | { state: "noService" }
  | { state: "unreadable" }
  | { state: "noAnswer" };

export interface BluetoothDevice {
  name: string;
  kind: BluetoothKind;
  connected: boolean;
  battery: BluetoothBattery;
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

/** Why the watchdog called an app runaway; each variant carries the reading that justified it. */
export type RunawayReason =
  | { reason: "cpuSustained"; sustainedSeconds: number }
  | { reason: "projectGone"; project: string }
  | { reason: "memoryCeiling"; memoryBytes: number; ceilingBytes: number };

export interface RunawayVerdict {
  /** `runaway:{appKey}:{day}` — dismissal scoped to one app for the rest of the day. */
  id: string;
  appKey: string;
  appName: string;
  reason: RunawayReason;
}

/**
 * The answer to quit_app. `error` is a code - "no-reading", "gone", "not-an-application",
 * "protected", "no-window", "denied" - or null; the UI owns the words.
 */
export interface QuitResult {
  asked: number;
  forceUsed: boolean;
  error: string | null;
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
  /** Whether Windows is asked not to sleep while Meerkat runs. */
  keepAwake: boolean;
  tray: TrayReadout[];
  panel: { layout: PanelLayout; tiles: SectionConfig[] };
  window: { tabs: TabConfig[] };
  alerts: AlertThresholds;
  /** Whether AI assistants may read Meerkat over MCP. */
  mcpEnabled: boolean;
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

export const TAB_ORDER = [
  "overview", "history", "cpu", "memory", "disk", "network", "gpu", "battery", "sound", "bluetooth", "perf",
] as const;
export type TabId = (typeof TAB_ORDER)[number];

/**
 * A persisted id that the panel can actually draw. Tabs and metrics share six of their names, so
 * a settings file written by an older default can carry "sound" where a tile should be — and a
 * tile with no series behind it does not render empty, it takes the whole panel down.
 */
export function isMetricKind(id: string): id is MetricKind {
  return (METRIC_ORDER as string[]).includes(id);
}

/**
 * The grouping engine folds every process it cannot open into one row under this key. Its
 * `name` arrives empty, so the label comes from the translation rather than from Windows.
 */
export const UNREADABLE_GROUP = "unreadable-processes";

export type PowerTabMode = "battery" | "thermal" | "hidden";

/**
 * What the power tab should be on this machine.
 *
 * Hiding it outright on a desktop would strand the temperature panel, which on many machines
 * is the only thermal reading an unprivileged process can get at all — so the tab keeps its
 * temperature content and changes its name instead. It disappears only when there is neither a
 * battery nor a single temperature to show.
 */
export function powerTabMode(s: Snapshot | null): PowerTabMode {
  if (!s) return "battery";
  if (s.battery.hasBattery) return "battery";
  return s.temperature.hottest !== null || s.temperature.sensors.length > 0 ? "thermal" : "hidden";
}

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

export type AudioRole = "render" | "capture";

export interface AudioEndpoint {
  id: string;
  friendlyName: string;
  volume: number;
  muted: boolean;
  peak: number;
  isDefault: boolean;
}

export interface AudioSession {
  /** Valid only until the next poll; writes re-resolve the session by identity. */
  id: number;
  pid: number;
  displayName: string;
  volume: number;
  muted: boolean;
  peak: number;
  active: boolean;
  role: AudioRole;
}

export interface AudioState {
  outputEndpoints: AudioEndpoint[];
  inputEndpoints: AudioEndpoint[];
  render: AudioSession[];
  capture: AudioSession[];
}

/** The WSL virtual machine's memory, as Linux inside it accounts for it. */
export interface WslMemory {
  distros: string[];
  totalBytes: number;
  usedBytes: number;
  /** File cache and reclaimable memory Linux gives back when asked. */
  cacheBytes: number;
  freeBytes: number;
}

/** One box of a storage map: a directory's share of the volume, or the free or unknown region. */
export interface StorageMapBox {
  /** The last path component, for the label; empty for the free and unknown regions. */
  name: string;
  path: string;
  bytes: number;
  /** Share of the whole volume, `bytes / totalBytes`. */
  share: number;
  /** Space the walk could not read; drawn apart from the named directories. */
  unknown: boolean;
  /** The volume's free space; drawn unfilled. */
  free: boolean;
}

/** A whole volume drawn as boxes: what is using the space, what is free, and what could not be read. */
export interface StorageMap {
  volumeId: string;
  totalBytes: number;
  usedBytes: number;
  freeBytes: number;
  /** The bytes the walk actually measured, before the boxes were cut to the top N. */
  measuredBytes: number;
  /** Directories that could not be opened; their sizes are part of the unknown region. */
  unreadable: number;
  scannedFiles: number;
  /** A newer scan or a cancel took over before this one finished. */
  cancelled: boolean;
  boxes: StorageMapBox[];
}

/** One progress update from a storage-map walk in flight. */
export interface StorageMapProgress {
  volumeId: string;
  files: number;
  bytes: number;
  unreadable: number;
}

// ---- Game performance sessions. Mirrors src-tauri/src/models.rs. ------------------------------

export type PerfTargetMode = "foreground" | "app";
export type FpsSource = "none" | "presentMon";
export type PerfVerdict = "healthy" | "warning" | "problem" | "inconclusive";
export type PerfIssueKind =
  | "fpsLow"
  | "fpsUnstable"
  | "cpuBound"
  | "gpuBound"
  | "vramPressure"
  | "gpuHot"
  | "memoryPressure"
  | "fpsUnavailable";

export interface PerfTarget {
  mode: PerfTargetMode;
  appKey: string | null;
  name: string;
  currentAppKey: string | null;
}

export interface PerfSample {
  atMs: number;
  fps: number | null;
  fpsSource: FpsSource;
  cpuPercent: number;
  appCpuPercent: number | null;
  cpuTempC: number | null;
  gpuPercent: number | null;
  appGpuPercent: number | null;
  gpuTempC: number | null;
  memoryPercent: number;
  appMemoryBytes: number | null;
  vramPercent: number | null;
  vramUsedBytes: number | null;
  vramTotalBytes: number | null;
}

export interface PerfIssue {
  kind: PerfIssueKind;
  verdict: PerfVerdict;
  /** Translation key; the backend never sends prose into the UI. */
  messageKey: string;
  metric: string;
  value: number | null;
}

export interface PerfStat {
  key: string;
  available: boolean;
  min: number | null;
  avg: number | null;
  max: number | null;
  p95: number | null;
  last: number | null;
}

export interface PerfReport {
  sessionId: string;
  verdict: PerfVerdict;
  headlineKey: string;
  issues: PerfIssue[];
  stats: PerfStat[];
  durationSeconds: number;
  sampleCount: number;
}

export interface PerfSession {
  id: string;
  target: PerfTarget;
  startedAtMs: number;
  endedAtMs: number | null;
  samples: PerfSample[];
  report: PerfReport | null;
}

export interface PerfSessionSummary {
  id: string;
  targetName: string;
  mode: PerfTargetMode;
  startedAtMs: number;
  endedAtMs: number | null;
  sampleCount: number;
  verdict: PerfVerdict;
}

export interface PerfStatus {
  active: boolean;
  sessionId: string | null;
  targetName: string;
  mode: PerfTargetMode;
  currentAppKey: string | null;
  sampleCount: number;
  startedAtMs: number | null;
  latest: PerfSample | null;
}
