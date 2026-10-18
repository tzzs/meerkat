import { useMemo } from "react";
import { ipc } from "../lib/ipc";
import { useLoaded } from "../lib/use-loaded";
import { visibleSections } from "../lib/sections";
import { useStore } from "../lib/store";
import type { BluetoothBattery, BluetoothDevice } from "../lib/types";
import { Card, Meter, Sections } from "./ui";

/**
 * The Bluetooth tab: what is connected, and what can honestly be said about each battery.
 *
 * A percentage only exists for devices that expose the standard Battery Service. For the rest —
 * AirPods, most headsets, anything behind a 2.4 GHz receiver — the row says why instead of
 * printing a zero, because a zero is a claim about a battery nobody read.
 */
export function BluetoothTab() {
  const { settings, t } = useStore();
  const order = visibleSections(settings, "bluetooth");
  // Two minutes: a battery moves by a percent or two over hours, and every ask opens a radio
  // link, which costs the very battery being reported.
  const devices = useLoaded(() => ipc.bluetoothDevices(), [] as BluetoothDevice[], [], 120_000);

  const rows = useMemo(() => [...devices].sort(byBattery), [devices]);
  const reading = rows.filter((d) => d.battery.state === "percent").length;

  return (
    <div className="stack">
      <Sections
        order={order}
        parts={{
          devices: (
            <Card
              title={t("bluetooth.devices")}
              right={
                rows.length > 0 ? (
                  <span className="faint num">
                    {reading}/{rows.length}
                  </span>
                ) : undefined
              }
            >
              {rows.length === 0 ? (
                <p className="empty faint">{t("bluetooth.empty")}</p>
              ) : (
                <div className="rows">
                  {rows.map((device) => (
                    <div className="row bt-row" key={`${device.kind}:${device.name}`}>
                      <span className="bt-kind">{device.kind === "le" ? "LE" : "BT"}</span>
                      <div className="row-main">
                        <span className="row-name">{device.name || t("bluetooth.classic")}</span>
                        <span className="row-sub">
                          {device.kind === "le" ? t("bluetooth.le") : t("bluetooth.classic")}
                          {" · "}
                          {/* Paired is not the same as connected, and Windows reports the two
                              differently: a controller that is switched off still lists here. */}
                          {device.connected ? t("bluetooth.connected") : t("bluetooth.paired")}
                          {" · "}
                          {batteryNote(device.battery, t)}
                        </span>
                      </div>
                      <div className="row-meter">
                        {device.battery.state === "percent" && (
                          <Meter ratio={device.battery.percent / 100} color="var(--m-bluetooth, var(--m-network))" />
                        )}
                      </div>
                      <span className="row-value num">
                        {device.battery.state === "percent" ? `${device.battery.percent}%` : "—"}
                      </span>
                      <span />
                    </div>
                  ))}
                </div>
              )}
              <p className="hint faint">{t("bluetooth.help")}</p>
            </Card>
          ),
        }}
      />
    </div>
  );
}

/**
 * Devices with a reading first, lowest battery first — the one about to die is the one worth
 * seeing. Everything else follows by name, so the list does not reshuffle between reads.
 */
function byBattery(a: BluetoothDevice, b: BluetoothDevice): number {
  const pa = a.battery.state === "percent" ? a.battery.percent : null;
  const pb = b.battery.state === "percent" ? b.battery.percent : null;
  if (pa !== null && pb !== null) return pa - pb || a.name.localeCompare(b.name);
  if (pa !== null) return -1;
  if (pb !== null) return 1;
  return a.name.localeCompare(b.name);
}

/** Why there is no number, in the words the tab already has for it. */
function batteryNote(battery: BluetoothBattery, t: ReturnType<typeof useStore>["t"]): string {
  switch (battery.state) {
    case "percent":
      return `${battery.percent}%`;
    case "noService":
      return t("bluetooth.noService");
    case "unreadable":
      return t("bluetooth.unreadable");
    case "noAnswer":
      return t("bluetooth.noAnswer");
  }
}
