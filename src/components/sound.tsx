import { useEffect, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { useStore } from "../lib/store";
import type { AudioEndpoint, AudioSession, AudioState } from "../lib/types";
import { AppIcon, Card, Pill } from "./ui";

/**
 * The mixer. Windows gives every application its own audio session, so this needs no
 * elevation and no helper — unlike the Mac version, which has to ask for a privileged
 * helper to see the same thing.
 */
export function SoundTab() {
  const { t } = useStore();
  const [audio, setAudio] = useState<AudioState | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const dragging = useRef(false);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      if (dragging.current) return;
      const next = await ipc.audioState();
      if (!alive) return;
      if (next) {
        setAudio(next);
        setFailure(null);
      } else {
        setFailure(t("state.noData"));
      }
    };
    void load();
    const timer = window.setInterval(load, 1000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [t]);

  const hold = {
    onPointerDown: () => {
      dragging.current = true;
    },
    onPointerUp: () => {
      dragging.current = false;
    },
    onPointerCancel: () => {
      dragging.current = false;
    },
  };

  if (!audio) {
    return (
      <div className="stack">
        <Card title={t("sound.title")}>
          <p className="empty">{failure ?? t("state.noData")}</p>
        </Card>
      </div>
    );
  }

  const outputs = audio.outputEndpoints;
  const inputs = audio.inputEndpoints;

  return (
    <div className="stack">
      <Card title={t("sound.output")} right={<Pill>{outputs.length}</Pill>}>
        <div className="mix-list">
          <MixHead />
          {outputs.map((endpoint) => (
            <EndpointRow key={endpoint.id} endpoint={endpoint} hold={hold} />
          ))}
          {outputs.length === 0 && <p className="empty">{t("state.noData")}</p>}
        </div>
        <p className="hint faint">{t("sound.persistHelp")}</p>
      </Card>

      <Card title={t("sound.apps")} right={<Pill>{audio.render.length}</Pill>}>
        <div className="mix-list">
          <MixHead />
          {audio.render.map((session) => (
            <SessionRow key={`${session.id}-${session.pid}`} session={session} hold={hold} />
          ))}
          {audio.render.length === 0 && <p className="empty">{t("sound.noSessions")}</p>}
        </div>
      </Card>

      <Card title={t("sound.input")} right={<Pill>{audio.capture.length}</Pill>}>
        <div className="mix-list">
          <MixHead />
          {inputs.map((endpoint) => (
            <EndpointRow key={endpoint.id} endpoint={endpoint} hold={hold} />
          ))}
          {audio.capture.map((session) => (
            <SessionRow key={`${session.id}-${session.pid}`} session={session} hold={hold} />
          ))}
          {inputs.length === 0 && audio.capture.length === 0 && <p className="empty">{t("state.noData")}</p>}
        </div>
        <p className="hint faint">{t("sound.micHelp")}</p>
      </Card>
    </div>
  );
}

/**
 * A level meter, deliberately not shaped like the volume slider next to it: no track,
 * no thumb, thinner, and the column is labelled. An unlabelled grey pill read as a
 * broken slider.
 */
function MixHead() {
  const { t } = useStore();
  return (
    <div className="mix-row mix-head" aria-hidden="true">
      <span />
      <span className="mix-label" />
      <span className="peak-cap">{t("sound.level")}</span>
      <span>{t("sound.volume")}</span>
      <span />
      <span className="mix-value">%</span>
    </div>
  );
}

function PeakBar({ peak }: { peak: number }) {
  return (
    <span className="peak" aria-hidden="true">
      <span
        className="peak-fill"
        style={{
          width: `${Math.min(100, Math.round(Math.sqrt(Math.max(0, peak)) * 100))}%`,
        }}
      />
    </span>
  );
}

function EndpointRow({
  endpoint,
  hold,
}: {
  endpoint: AudioEndpoint;
  hold: Record<string, () => void>;
}) {
  const { t } = useStore();
  const [busy, setBusy] = useState(false);
  return (
    <div className="mix-row">
      <span className="mono-avatar" aria-hidden="true" />
      <span className="mix-name">
        <span className="mix-label">{endpoint.friendlyName}</span>
        {endpoint.isDefault && <Pill tone="accent">{t("sound.default")}</Pill>}
      </span>
      <PeakBar peak={endpoint.peak} />
      <input
        type="range"
        min={0}
        max={100}
        value={Math.round(endpoint.volume * 100)}
        disabled={busy}
        aria-label={t("sound.master")}
        {...hold}
        onChange={(event) => {
          void (async () => {
            setBusy(true);
            const value = Number(event.target.value) / 100;
            const error = await ipc.audioSetEndpoint(endpoint.id, value, endpoint.muted);
            if (typeof error === "string") {
              // The command returns Err as a rejected invoke; surface it rather than
              // leaving the slider to silently snap back.
              console.error(error);
            }
            setBusy(false);
          })();
        }}
      />
      <button
        className={`mute ${endpoint.muted ? "on" : ""}`}
        title={endpoint.muted ? t("sound.muted") : t("sound.master")}
        onClick={() => {
          void ipc.audioSetEndpoint(endpoint.id, endpoint.volume, !endpoint.muted);
        }}
      >
        {endpoint.muted ? "⊘" : "♪"}
      </button>
      <span className="mix-value num">{Math.round(endpoint.volume * 100)}%</span>
    </div>
  );
}

function SessionRow({
  session,
  hold,
}: {
  session: AudioSession;
  hold: Record<string, () => void>;
}) {
  const { t, snapshot } = useStore();
  const [busy, setBusy] = useState(false);
  // Core Audio names a session after its executable; the grouping already knows the app
  // the user actually sees, so prefer that over `deltaforceclient-win64-shipping.exe`.
  const owner = snapshot?.apps.find((app) => app.pids.includes(session.pid));
  const name = owner?.name?.trim() || session.displayName?.trim() || `pid ${session.pid}`;
  return (
    <div className={`mix-row ${session.active ? "" : "inactive"}`}>
      <AppIcon name={name} seed={String(session.pid)} exePath={owner?.exePath || undefined} />
      <span className="mix-name">
        <span className="mix-label">{name}</span>
        {!session.active && <Pill>{t("label.standby")}</Pill>}
      </span>
      <PeakBar peak={session.peak} />
      <input
        type="range"
        min={0}
        max={100}
        value={Math.round(session.volume * 100)}
        disabled={busy}
        aria-label={`${t("sound.master")} ${name}`}
        {...hold}
        onChange={(event) => {
          void (async () => {
            setBusy(true);
            await ipc.audioSetVolume(session.role, session.id, session.pid, Number(event.target.value) / 100, session.muted);
            setBusy(false);
          })();
        }}
      />
      <button
        className={`mute ${session.muted ? "on" : ""}`}
        title={session.muted ? t("sound.muted") : t("action.cancel")}
        onClick={() => {
          void ipc.audioSetVolume(session.role, session.id, session.pid, session.volume, !session.muted);
        }}
      >
        {session.muted ? "⊘" : "♪"}
      </button>
      <span className="mix-value num">{Math.round(session.volume * 100)}%</span>
    </div>
  );
}
