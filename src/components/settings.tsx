import { useEffect, useId, useState, type ReactNode } from "react";
import { ipc } from "../lib/ipc";
import { useStore, fallbackSettings } from "../lib/store";
import { formatBytes, formatRate, formatTemp } from "../lib/format";
import { nudge, parseNumberInput, type NumberProblem, type NumberRule } from "../lib/number-input";
import { effectiveSections, moveWithin, sectionLabelKey, withTabSections } from "../lib/sections";
import { alertFieldSpec, coresForPercent, toShown, toStored, type AlertField } from "../lib/thresholds";
import {
  isMetricKind,
  METRIC_ORDER,
  powerTabMode,
  type AppRule,
  type MetricKind,
  type SectionConfig,
  type Settings,
  type TrayReadout,
  type TrayStyle,
} from "../lib/types";
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
import { ToggleChip, ToggleGroup } from "./ui/toggle-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";
import { TAB_LABEL } from "../i18n";
import type { Key } from "../i18n/en";

type Page = "general" | "layout" | "about";

const TRAY_METRICS: Record<MetricKind, Key> = {
  cpu: "metric.cpu",
  memory: "metric.memory",
  disk: "metric.disk",
  network: "metric.network",
  gpu: "metric.gpu",
  battery: "metric.battery",
  temperature: "metric.temperature",
};

export function SettingsPages({ page, setPage }: { page: Page; setPage: (p: Page | null) => void }) {
  const { settings, save, t, settingsError, clearSettingsError } = useStore();
  const [confirmReset, setConfirmReset] = useState(false);
  const pages: [Page, string][] = [
    ["general", t("settings.general")],
    ["layout", t("settings.layout")],
    ["about", t("settings.about")],
  ];
  return (
    <div className="settings">
      <aside className="settings-nav">
        {pages.map(([id, label]) => (
          <button key={id} className={page === id ? "on" : ""} onClick={() => setPage(id)}>
            {label}
          </button>
        ))}
      </aside>
      <div className="settings-body">
        {/* A refused save has already been rolled back; this says why the control moved. */}
        {settingsError && (
          <div className="settings-error" role="alert">
            <span className="dot" />
            <span className="settings-error-text">
              {t("settings.saveFailed")}
              <span className="faint"> {settingsError}</span>
            </span>
            <button onClick={clearSettingsError} aria-label={t("action.dismiss")}>
              ×
            </button>
          </div>
        )}
        {page === "general" && <General settings={settings} save={save} />}
        {page === "layout" && <Layout settings={settings} save={save} />}
        {page === "about" && <About />}
        <button className="reset" onClick={() => setConfirmReset(true)}>
          {t("action.reset")}
        </button>
      </div>
      {/* It used to wipe the layout, the units and the thresholds on a single click. */}
      <Confirm
        open={confirmReset}
        onClose={() => setConfirmReset(false)}
        title={t("settings.resetTitle")}
        body={t("settings.resetBody")}
        confirmLabel={t("action.reset")}
        onConfirm={() => save({ ...fallbackSettings, tray: settings.tray })}
      />
    </div>
  );
}

/**
 * A question with two answers, over Radix's dialog (see ui/dialog): focus is held inside, Escape
 * and the backdrop cancel, and focus starts on Cancel, so Enter on an open dialog does nothing
 * irreversible.
 */
function Confirm({
  open,
  onClose,
  title,
  body,
  confirmLabel,
  danger = false,
  onConfirm,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  body: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
}) {
  const { t } = useStore();
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent label={title}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className="text-muted-foreground">{body}</DialogDescription>
        </DialogHeader>
        <DialogActions>
          <DialogClose>
            <Button variant="chip" data-autofocus>
              {t("action.cancel")}
            </Button>
          </DialogClose>
          <span className="spacer" />
          <Button
            variant={danger ? "danger" : "primary"}
            onClick={() => {
              onClose();
              onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </DialogActions>
      </DialogContent>
    </Dialog>
  );
}

function Row({ label, children, hint }: { label: string; children?: ReactNode; hint?: string }) {
  return (
    <label className="row-setting">
      <span className="row-label">{label}</span>
      <span className="row-control">{children}</span>
      {hint && <span className="row-hint faint">{hint}</span>}
    </label>
  );
}

function select<T extends string>(
  value: T,
  options: [T, string][],
  onChange: (v: T) => void,
  opts?: { className?: string; label?: string },
) {
  return (
    <Select value={value} onValueChange={(next) => onChange(next as T)}>
      {/* Inside a row the label element names it; in a tray row there is no label to borrow,
          so the readout's own name is passed along. */}
      <SelectTrigger className={opts?.className} aria-label={opts?.label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map(([v, label]) => (
          <SelectItem key={v} value={v}>
            {label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function toggle(value: boolean, onChange: (v: boolean) => void) {
  return <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} />;
}

/** "5" for a quarter of twenty cores, "0.6" for three per cent of them. */
function formatCores(cores: number, locale: string): string {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: cores < 10 ? 1 : 0 }).format(cores);
}

function General({ settings, save }: { settings: Settings; save: (s: Settings) => void }) {
  const { t, snapshot, units } = useStore();
  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => save({ ...settings, [key]: value });
  const sample = snapshot;
  const logicalCores = snapshot?.machine.logicalCores ?? null;
  const cores = coresForPercent(settings.alerts.cpuPercent, logicalCores);

  /** A threshold's box, in the units the lists print it in (see alertFieldSpec). */
  const alertBox = (field: AlertField) => {
    const spec = alertFieldSpec(field, settings);
    return (
      <NumberBox
        value={toShown(settings.alerts[field], spec)}
        suffix={spec.suffix}
        rule={spec.rule}
        onCommit={(shown) =>
          save({ ...settings, alerts: { ...settings.alerts, [field]: toStored(shown, spec) } })
        }
      />
    );
  };

  return (
    <div className="settings-group">
      <h4 className="section-title">{t("settings.units")}</h4>
      <Row label={t("settings.temperatureUnit")} hint={sample?.temperature.hottest ? formatTemp(sample.temperature.hottest, { ...units, temperature: settings.temperatureUnit }) : undefined}>
        {select(settings.temperatureUnit, [["celsius", "°C"], ["fahrenheit", "°F"]], (v) => set("temperatureUnit", v))}
      </Row>
      <Row
        label={t("settings.networkUnit")}
        hint={sample ? formatRate(sample.network.downloadBytesPerSecond, { ...units, network: settings.networkUnit }, "network") : undefined}
      >
        {select(settings.networkUnit, [["bytes", t("settings.bytes")], ["bits", t("settings.bits")]], (v) => set("networkUnit", v))}
      </Row>
      <Row
        label={t("settings.storageUnit")}
        hint={sample ? formatBytes(sample.memory.inUseBytes, { ...units, storage: settings.storageUnit }) : undefined}
      >
        {select(settings.storageUnit, [["decimal", t("settings.decimal")], ["binary", t("settings.binary")]], (v) => set("storageUnit", v))}
      </Row>
      <Row label={t("settings.cpuScale")}>
        {select(
          settings.cpuScale,
          [
            ["perMachine", t("settings.cpuPerMachine")],
            ["perCore", t("settings.cpuPerCore")],
          ],
          (v) => set("cpuScale", v),
        )}
      </Row>

      <h4 className="section-title">{t("app.name")}</h4>
      <Row label={t("settings.language")}>
        {select(
          settings.language,
          [
            ["auto", t("settings.languageAuto")],
            ["en", "English"],
            ["zhHans", "简体中文"],
          ],
          (v) => set("language", v),
        )}
      </Row>
      <Row label={t("settings.theme")}>
        {select(
          settings.theme,
          [
            ["auto", t("settings.themeAuto")],
            ["light", t("settings.themeLight")],
            ["dark", t("settings.themeDark")],
          ],
          (v) => set("theme", v),
        )}
      </Row>
      <Row label={t("settings.showSystemApps")}>{toggle(settings.showSystemAppsInLists, (v) => set("showSystemAppsInLists", v))}</Row>
      <Row label={t("settings.openWindowOn")}>
        {select(
          settings.openWindowOn,
          [
            ["overview", t("tab.overview")],
            ["last", t("settings.openWindowLast")],
            // Radix rejects two items with the same value where a native select just showed the
            // first of them, so the tab list stops repeating overview.
            ...settings.window.tabs
              .filter((x) => x.visible && x.id !== "overview")
              .map((x) => [x.id, t(TAB_LABEL[x.id] ?? "tab.overview")] as [string, string]),
          ],
          (v) => set("openWindowOn", v),
        )}
      </Row>
      <Row label={t("settings.startAtLogin")}>{toggle(settings.startAtLogin, (v) => set("startAtLogin", v))}</Row>
      <Row label={t("settings.showInTaskbar")}>{toggle(settings.showInTaskbar, (v) => set("showInTaskbar", v))}</Row>

      <h4 className="section-title">{t("settings.alerts")}</h4>
      <Row label={t("settings.alertsEnabled")}>
        {toggle(settings.alerts.enabled, (v) => save({ ...settings, alerts: { ...settings.alerts, enabled: v } }))}
      </Row>
      {/* The threshold is a share of the whole machine, which on a twenty-thread chip is five
          threads; the hint says so, because "25%" alone reads as a quarter of one core. */}
      <Row
        label={t("settings.alertCpuMachine")}
        hint={
          cores === null || logicalCores === null
            ? undefined
            : t("settings.alertCpuCores", { cores: formatCores(cores, units.locale), total: logicalCores })
        }
      >
        {alertBox("cpuPercent")}
      </Row>
      <Row label={t("settings.alertMemoryWindow")}>{alertBox("memoryGrowBytes")}</Row>
      <Row label={t("settings.alertDisk")}>{alertBox("diskBytesPerSecond")}</Row>
      <Row label={t("settings.alertNetwork")}>{alertBox("networkBytesPerSecond")}</Row>
      <Row label={t("settings.alertSustained")}>{alertBox("sustainedSeconds")}</Row>

      <RulesList />
      <McpSection />
    </div>
  );
}

/**
 * A number the user types, saved only when it is valid for the field.
 *
 * Enter commits, so does leaving the box; Escape puts the saved value back (and, with nothing
 * to put back, is left to close the page as it always did); the arrow keys step. What fails the
 * field's rule is said in place and is never sent to the backend.
 */
function NumberBox({
  value,
  suffix,
  rule,
  onCommit,
}: {
  value: number;
  suffix: string;
  rule: NumberRule;
  onCommit: (value: number) => void;
}) {
  const { t } = useStore();
  const [text, setText] = useState(String(value));
  const [problem, setProblem] = useState<NumberProblem | null>(null);
  const errorId = useId();
  // The saved value moved (a rollback, a unit change, the backend clamped it): follow it.
  useEffect(() => {
    setText(String(value));
    setProblem(null);
  }, [value]);

  const commit = () => {
    const parsed = parseNumberInput(text, rule);
    if (!parsed.ok) {
      setProblem(parsed.problem);
      return;
    }
    setProblem(null);
    setText(String(parsed.value));
    if (parsed.value !== value) onCommit(parsed.value);
  };

  const edited = text !== String(value) || problem !== null;
  const error =
    problem === null
      ? null
      : t(problem === "notWhole" ? "settings.numberWhole" : "settings.numberRange", {
          min: rule.min,
          max: rule.max,
        });

  return (
    <span className="number-box">
      <input
        value={text}
        inputMode={rule.integer ? "numeric" : "decimal"}
        aria-invalid={problem !== null}
        aria-describedby={error ? errorId : undefined}
        onChange={(event) => {
          setText(event.target.value);
          setProblem(null);
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape" && edited) {
            // Handled here: it would otherwise also close the whole Settings page.
            event.preventDefault();
            event.stopPropagation();
            setText(String(value));
            setProblem(null);
          } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            event.preventDefault();
            setText(nudge(text, event.key === "ArrowUp" ? 1 : -1, rule));
            setProblem(null);
          }
        }}
      />
      <span className="faint">{suffix}</span>
      {error && (
        <span id={errorId} className="field-error" role="alert">
          {error}
        </span>
      )}
    </span>
  );
}

function Layout({ settings, save }: { settings: Settings; save: (s: Settings) => void }) {
  const { t, snapshot, units } = useStore();
  // One tab's sections are open at a time; the list is long enough with all eight shut.
  const [openTab, setOpenTab] = useState<string | null>(null);

  const updateTray = (next: TrayReadout[]) => save({ ...settings, tray: next });

  const move = (index: number, delta: number) => {
    const list = [...settings.tray];
    const target = index + delta;
    if (target < 0 || target >= list.length) return;
    [list[index], list[target]] = [list[target], list[index]];
    updateTray(list);
  };

  const addReadout = (metric: MetricKind) => {
    const readout: TrayReadout = {
      id: `${metric}-${settings.tray.length}`,
      metric,
      style: "value",
      showIcon: false,
      showCaption: false,
      invert: false,
      enabled: true,
    };
    updateTray([...settings.tray, readout]);
  };

  const usedMetrics = new Set(settings.tray.map((r) => r.metric));

  // The panel can only draw a metric, so only metrics are offered; an id left in the file by an
  // older default stays where it is and is stepped over.
  const tiles = settings.panel.tiles;
  const metricTiles = tiles.filter((tile) => isMetricKind(tile.id));
  const saveTiles = (next: SectionConfig[]) => save({ ...settings, panel: { ...settings.panel, tiles: next } });
  const moveTile = (id: string, delta: -1 | 1) =>
    saveTiles(moveWithin(tiles, id, delta, (tile) => isMetricKind(tile.id)));

  const saveTabs = (tabs: Settings["window"]["tabs"]) => save({ ...settings, window: { ...settings.window, tabs } });
  const moveTab = (id: string, delta: -1 | 1) => saveTabs(moveWithin(settings.window.tabs, id, delta));

  return (
    <div className="settings-group">
      <h4 className="section-title">{t("settings.tray")}</h4>
      <p className="hint faint">{t("settings.trayHelp")}</p>
      {settings.tray.map((readout, index) => (
        <div className="tray-row" key={readout.id}>
          <input
            type="checkbox"
            checked={readout.enabled}
            aria-label={t(TRAY_METRICS[readout.metric])}
            onChange={(e) =>
              updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, enabled: e.target.checked } : r)))
            }
          />
          <span className="tray-name">{t(TRAY_METRICS[readout.metric])}</span>
          {select(
            readout.style,
            [
              ["value", t("settings.styleValue")],
              ["graph", t("settings.styleGraph")],
              ["both", t("settings.styleBoth")],
            ] as [TrayStyle, string][],
            (v) => updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, style: v } : r))),
            {
              className: "min-w-0 rounded-xs px-1.5 py-[3px] text-[11.5px]",
              label: t(TRAY_METRICS[readout.metric]),
            },
          )}
          {readout.metric === "memory" && (
            <label className="inline-check">
              <input
                type="checkbox"
                checked={readout.invert}
                onChange={(e) =>
                  updateTray(settings.tray.map((r) => (r.id === readout.id ? { ...r, invert: e.target.checked } : r)))
                }
              />
              {t("label.available")}
            </label>
          )}
          <span className="tray-preview num">{preview(readout, snapshot, units)}</span>
          <span className="spacer" />
          <button disabled={index === 0} onClick={() => move(index, -1)} aria-label={t("settings.moveUp")}>
            ↑
          </button>
          <button
            disabled={index === settings.tray.length - 1}
            onClick={() => move(index, 1)}
            aria-label={t("settings.moveDown")}
          >
            ↓
          </button>
          <button
            onClick={() => updateTray(settings.tray.filter((r) => r.id !== readout.id))}
            aria-label={t("action.cancel")}
          >
            ×
          </button>
        </div>
      ))}
      <div className="add-row">
        <span>{t("settings.addReadout")}</span>
        {METRIC_ORDER.filter((m) => !usedMetrics.has(m) && (m !== "battery" || snapshot?.battery.hasBattery)).map((metric) => (
          <Button key={metric} variant="chip" onClick={() => addReadout(metric)}>
            {t(TRAY_METRICS[metric])}
          </Button>
        ))}
      </div>

      <h4 className="section-title">{t("settings.panel")}</h4>
      <Row label={t("settings.panelLayout")}>
        {select(
          settings.panel.layout,
          [
            ["tiles", t("settings.panelTiles")],
            ["list", t("settings.panelList")],
          ],
          (v) => save({ ...settings, panel: { ...settings.panel, layout: v } }),
        )}
      </Row>
      {/* One group, so the chip that shows a tile still names it to a screen reader as part of
          "Tiles shown in the panel"; the arrows beside each chip set the order the panel draws. */}
      <ToggleGroup
        type="multiple"
        className="mb-[var(--sp-3)] flex-col items-stretch gap-0"
        value={metricTiles.filter((tile) => tile.visible).map((tile) => tile.id)}
        onValueChange={(next) =>
          saveTiles(tiles.map((tile) => (isMetricKind(tile.id) ? { ...tile, visible: next.includes(tile.id) } : tile)))
        }
        aria-label={t("settings.visibleTiles")}
      >
        {metricTiles.map((tile, index) => {
          const name = t(TRAY_METRICS[tile.id as MetricKind]);
          return (
            <div className="tray-row" key={tile.id}>
              <ToggleChip value={tile.id}>{name}</ToggleChip>
              <span className="spacer" />
              <button
                disabled={index === 0}
                onClick={() => moveTile(tile.id, -1)}
                aria-label={`${t("settings.moveUp")}: ${name}`}
              >
                ↑
              </button>
              <button
                disabled={index === metricTiles.length - 1}
                onClick={() => moveTile(tile.id, 1)}
                aria-label={`${t("settings.moveDown")}: ${name}`}
              >
                ↓
              </button>
            </div>
          );
        })}
      </ToggleGroup>

      <h4 className="section-title">{t("settings.windowTabs")}</h4>
      {settings.window.tabs.map((tab, index) => {
        // The power tab is not a choice on hardware that has nothing to put in it.
        const mode = powerTabMode(snapshot);
        const offHardware = tab.id === "battery" && mode === "hidden";
        const label = tab.id === "battery" && mode === "thermal" ? "metric.temperature" : TAB_LABEL[tab.id];
        const name = t(label);
        const sections = effectiveSections(tab, tab.id);
        const open = openTab === tab.id;
        const setSections = (next: SectionConfig[]) => save(withTabSections(settings, tab.id, next));
        return (
          <div className="tab-config" key={tab.id}>
            <div className="tray-row">
              <input
                type="checkbox"
                checked={tab.visible}
                disabled={offHardware}
                aria-label={name}
                onChange={(e) =>
                  saveTabs(settings.window.tabs.map((x, i) => (i === index ? { ...x, visible: e.target.checked } : x)))
                }
              />
              <span className="tray-name">{name}</span>
              {offHardware && <span className="faint">{t("settings.noPowerHardware")}</span>}
              <span className="spacer" />
              {sections.length > 0 && (
                <button
                  className="tray-expand"
                  aria-expanded={open}
                  onClick={() => setOpenTab(open ? null : tab.id)}
                >
                  {t("settings.sections")} {open ? "▴" : "▾"}
                </button>
              )}
              <button
                disabled={index === 0}
                onClick={() => moveTab(tab.id, -1)}
                aria-label={`${t("settings.moveUp")}: ${name}`}
              >
                ↑
              </button>
              <button
                disabled={index === settings.window.tabs.length - 1}
                onClick={() => moveTab(tab.id, 1)}
                aria-label={`${t("settings.moveDown")}: ${name}`}
              >
                ↓
              </button>
            </div>
            {open && (
              <div className="section-list">
                {sections.map((section, i) => {
                  const key = sectionLabelKey(tab.id, section.id);
                  const sectionName = key ? t(key) : section.id;
                  return (
                    <div className="tray-row" key={section.id}>
                      <input
                        type="checkbox"
                        checked={section.visible}
                        aria-label={sectionName}
                        onChange={(e) =>
                          setSections(
                            sections.map((s) => (s.id === section.id ? { ...s, visible: e.target.checked } : s)),
                          )
                        }
                      />
                      <span className="tray-name">{sectionName}</span>
                      <span className="spacer" />
                      <button
                        disabled={i === 0}
                        onClick={() => setSections(moveWithin(sections, section.id, -1))}
                        aria-label={`${t("settings.moveUp")}: ${sectionName}`}
                      >
                        ↑
                      </button>
                      <button
                        disabled={i === sections.length - 1}
                        onClick={() => setSections(moveWithin(sections, section.id, 1))}
                        aria-label={`${t("settings.moveDown")}: ${sectionName}`}
                      >
                        ↓
                      </button>
                    </div>
                  );
                })}
                {/* Back to "all of them, as designed", which is how the file stores it. */}
                <button className="reset-tab" disabled={tab.sections.length === 0} onClick={() => setSections([])}>
                  {t("settings.resetTab")}
                </button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function preview(
  readout: TrayReadout,
  snapshot: ReturnType<typeof useStore>["snapshot"],
  units: ReturnType<typeof useStore>["units"],
): string {
  if (!snapshot) return "—";
  switch (readout.metric) {
    case "cpu":
      return `${Math.round(snapshot.cpu.totalPercent)}%`;
    case "memory": {
      const ratio = snapshot.memory.inUseBytes / Math.max(1, snapshot.memory.totalBytes);
      return readout.invert ? `${Math.round((1 - ratio) * 100)}%` : `${Math.round(ratio * 100)}%`;
    }
    case "disk":
      return formatRate(snapshot.disk.readBytesPerSecond + snapshot.disk.writeBytesPerSecond, units, "storage");
    case "network":
      return formatRate(snapshot.network.downloadBytesPerSecond, units, "network");
    case "gpu":
      return snapshot.gpu.totalPercent === null ? "—" : `${Math.round(snapshot.gpu.totalPercent)}%`;
    case "battery":
      return snapshot.battery.hasBattery && snapshot.battery.chargePercent !== null
        ? `${Math.round(snapshot.battery.chargePercent)}%`
        : "—";
    case "temperature":
      return snapshot.temperature.hottest !== null ? `${Math.round(snapshot.temperature.hottest)}°` : "—";
  }
}

function About() {
  const { t, snapshot } = useStore();
  const [version, setVersion] = useState("…");
  const [copied, setCopied] = useState(false);
  const [confirmQuit, setConfirmQuit] = useState(false);
  useEffect(() => {
    void ipc.version().then(setVersion);
  }, []);

  return (
    <div className="settings-group">
      <Row label={t("settings.aboutVersion")}>
        <span className="num">{version}</span>
      </Row>
      <Row label={t("settings.aboutSampler")}>
        <span className="num">{snapshot ? `${(snapshot.samplerCostMicroseconds / 1000).toFixed(1)} ms` : "—"}</span>
      </Row>
      <Row label={t("settings.privacy")} />
      <Button
        variant="primary"
        onClick={async () => {
          const report = await ipc.copyReport();
          await navigator.clipboard.writeText(report).catch(() => undefined);
          setCopied(true);
          window.setTimeout(() => setCopied(false), 2000);
        }}
      >
        {copied ? t("action.done") : t("settings.aboutCopyReport")}
      </Button>
      <p className="hint faint">{t("settings.aboutReportHelp")}</p>

      {/* Closing the window only hides it, so without this the way out was the tray menu. */}
      <Button variant="danger" className="mt-[var(--sp-4)] self-start" onClick={() => setConfirmQuit(true)}>
        {t("action.quit")}
      </Button>
      <p className="hint faint">{t("settings.quitHelp")}</p>
      <Confirm
        open={confirmQuit}
        onClose={() => setConfirmQuit(false)}
        title={t("quit.title")}
        body={t("quit.body")}
        confirmLabel={t("action.quit")}
        danger
        onConfirm={() => ipc.quitMeerkat()}
      />
    </div>
  );
}

/**
 * Off by default. What an assistant reads may go to a model elsewhere, and command lines and
 * window titles can hold things worth keeping, so the switch says so next to itself.
 */
function McpSection() {
  const { settings, save, t } = useStore();
  const [command, setCommand] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => void ipc.mcpCommand().then(setCommand), []);
  return (
    <>
      <h4 className="section-title">{t("settings.mcp")}</h4>
      <Row label={t("settings.mcpEnabled")}>
        <input
          type="checkbox"
          checked={settings.mcpEnabled}
          onChange={(e) => save({ ...settings, mcpEnabled: e.target.checked })}
        />
      </Row>
      <p className="hint faint">{t("settings.mcpHelp")}</p>
      {settings.mcpEnabled && command && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded-sm bg-surface-2 px-2 py-1 font-num text-[11px]">{command}</code>
          <Button
            variant="chip"
            onClick={() =>
              void navigator.clipboard?.writeText(command).then(() => {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              })
            }
          >
            {copied ? t("detail.copied") : t("settings.mcpCopy")}
          </Button>
        </div>
      )}
    </>
  );
}

/**
 * The per-app rules Meerkat is keeping. They are set from an app's details, where the app is in
 * front of you; this is where to find them again, including for apps that are not running.
 */
function RulesList() {
  const { t } = useStore();
  const [rules, setRules] = useState<AppRule[]>([]);
  const load = () => void ipc.rules().then(setRules);
  useEffect(load, []);
  const describe = (rule: AppRule) =>
    [
      rule.efficiency ? t("control.efficiency") : null,
      rule.priority ? `${t("control.priority")}: ${t(`control.priority.${rule.priority}`)}` : null,
      rule.muteAlerts ? t("control.mute") : null,
    ]
      .filter(Boolean)
      .join(" · ");
  return (
    <>
      <h4 className="section-title">{t("settings.rules")}</h4>
      {rules.length === 0 ? (
        <p className="hint faint">{t("settings.rulesEmpty")}</p>
      ) : (
        rules.map((rule) => (
          <div className="tray-row" key={rule.key}>
            <span className="tray-name">{rule.name}</span>
            <span className="faint min-w-0 flex-1 truncate">{describe(rule)}</span>
            <Button
              variant="chip"
              onClick={() =>
                void ipc
                  .setRule({ ...rule, efficiency: false, priority: null, muteAlerts: false })
                  .then(load)
              }
            >
              {t("settings.rulesRemove")}
            </Button>
          </div>
        ))
      )}
    </>
  );
}
