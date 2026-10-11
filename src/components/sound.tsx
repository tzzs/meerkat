import { useEffect, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { visibleSections } from "../lib/sections";
import { useStore } from "../lib/store";
import type { AudioEndpoint, AudioSession, AudioState } from "../lib/types";
import { AppIcon, Card, Pill, Sections } from "./ui";
import { Button } from "./ui/button";
import { Hint } from "./ui/tooltip";
import { Slider } from "./ui/slider";

/** Pointer handlers that tell the poll to stand down while a fader is under the hand. */
type Hold = Pick<
  React.ComponentProps<typeof Slider>,
  "onPointerDown" | "onPointerUp" | "onPointerCancel"
>;

/**
 * The mixer. Windows gives every application its own audio session, so this needs no
 * elevation and no helper — unlike the Mac version, which has to ask for a privileged
 * helper to see the same thing.
 */
export function SoundTab() {
  const { t, settings } = useStore();
  const order = visibleSections(settings, "sound");
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
    // Four times a second rather than the snapshot's once: a level meter that updates once a
    // second looks broken even when it is accurate. Measured cost of one read is ~10ms on the
    // main thread, so this is about 4% of it while the tab is open and none at all otherwise.
    const timer = window.setInterval(load, 250);
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
      <Sections
        order={order}
        parts={{
          output: (
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
          ),
          apps: (
            <Card title={t("sound.apps")} right={<Pill>{audio.render.length}</Pill>}>
              <div className="mix-list">
                <MixHead />
                {audio.render.map((session) => (
                  <SessionRow key={`${session.id}-${session.pid}`} session={session} hold={hold} />
                ))}
                {audio.render.length === 0 && <p className="empty">{t("sound.noSessions")}</p>}
              </div>
            </Card>
          ),
          input: (
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
          ),
        }}
      />
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
      <span>{t("sound.levelVolume")}</span>
      <span />
      <span className="mix-value">%</span>
    </div>
  );
}

/**
 * The shown value follows the pointer, not the poll.
 *
 * The write is a round trip and the state that carries the real value only arrives a few times
 * a second, so a slider that waits for either lags behind the hand. The local echo is dropped
 * as soon as the poll agrees with it; if the write was refused the poll never agrees, and the
 * thumb stays where the user left it until something else changes it, which is the honest
 * reading of "we asked and Windows did not do it".
 */
function useOptimisticVolume(polled: number): { shown: number; setLocal: (n: number) => void } {
  const [local, setLocal] = useState<number | null>(null);
  useEffect(() => {
    if (local !== null && Math.abs(polled - local) < 0.006) setLocal(null);
  }, [polled, local]);
  return { shown: local ?? polled, setLocal };
}

/**
 * Coalesce a drag into one outstanding write.
 *
 * A fast drag fires a change per pointer sample; each Core Audio call takes about ten
 * milliseconds, so queueing them all would leave the mixer catching up for a second after the
 * hand stopped. Only the newest value is kept, and it goes out as a trailing write.
 */
function useWriteCoalescer(write: (next: number) => Promise<unknown>): (next: number) => void {
  const inFlight = useRef(false);
  const queued = useRef<number | null>(null);
  const latest = useRef(write);
  latest.current = write;

  const pump = () => {
    if (inFlight.current || queued.current === null) return;
    const next = queued.current;
    queued.current = null;
    inFlight.current = true;
    void latest.current(next).finally(() => {
      inFlight.current = false;
      pump();
    });
  };

  return (next: number) => {
    queued.current = next;
    pump();
  };
}

/** The volume slider with the level inside it. */
function Fader({
  value,
  peak,
  label,
  hold,
  onInput,
}: {
  value: number;
  peak: number;
  label: string;
  hold: Hold;
  onInput: (next: number) => void;
}) {
  // Square-rooted because dB is logarithmic: a linear meter reads silent most of the day.
  // Then scaled by the volume, because Core Audio reports the peak of the stream *before* the
  // slider attenuates it — drawn unscaled, a device at 23% showed a level that ran past its own
  // thumb, which reads as a control that is lying about its value.
  const level = Math.min(1, Math.sqrt(Math.max(0, peak))) * Math.min(1, Math.max(0, value));
  return (
    <Slider
      label={label}
      min={0}
      max={100}
      step={1}
      value={[Math.round(value * 100)]}
      onValueChange={([next]) => onInput(next / 100)}
      overlay={
        <span
          aria-hidden="true"
          className="absolute inset-0 origin-left bg-accent transition-transform duration-[90ms] ease-linear"
          style={{ transform: `scaleX(${level})` }}
        />
      }
      {...hold}
    />
  );
}

function EndpointRow({
  endpoint,
  hold,
}: {
  endpoint: AudioEndpoint;
  hold: Hold;
}) {
  const { t } = useStore();
  const { shown, setLocal } = useOptimisticVolume(endpoint.volume);
  const send = useWriteCoalescer(async (next) => {
    const error = await ipc.audioSetEndpoint(endpoint.id, next, endpoint.muted);
    if (typeof error === "string") {
      // The command returns Err as a rejected invoke; surface it rather than leaving the
      // slider to silently snap back.
      console.error(error);
    }
  });
  return (
    <div className="mix-row">
      <span className="mono-avatar" aria-hidden="true" />
      <span className="mix-name">
        <span className="mix-label">{endpoint.friendlyName}</span>
        {endpoint.isDefault && <Pill tone="accent">{t("sound.default")}</Pill>}
      </span>
      <Fader
        value={shown}
        peak={endpoint.peak}
        label={t("sound.master")}
        hold={hold}
        onInput={(next) => {
          setLocal(next);
          send(next);
        }}
      />
      <Hint label={endpoint.muted ? t("sound.muted") : t("sound.master")}>
        <Button
          variant="mute"
          on={endpoint.muted}
          aria-label={endpoint.muted ? t("sound.muted") : t("sound.master")}
          onClick={() => {
            void ipc.audioSetEndpoint(endpoint.id, endpoint.volume, !endpoint.muted);
          }}
        >
          {endpoint.muted ? "⊘" : "♪"}
        </Button>
      </Hint>
      <span className="mix-value num">{Math.round(shown * 100)}%</span>
    </div>
  );
}

function SessionRow({
  session,
  hold,
}: {
  session: AudioSession;
  hold: Hold;
}) {
  const { t, snapshot } = useStore();
  // Core Audio names a session after its executable; the grouping already knows the app
  // the user actually sees, so prefer that over `deltaforceclient-win64-shipping.exe`.
  const owner = snapshot?.apps.find((app) => app.pids.includes(session.pid));
  const name = owner?.name?.trim() || session.displayName?.trim() || `pid ${session.pid}`;
  const { shown, setLocal } = useOptimisticVolume(session.volume);
  const send = useWriteCoalescer((next) =>
    ipc.audioSetVolume(session.role, session.id, session.pid, next, session.muted),
  );
  return (
    <div className={`mix-row ${session.active ? "" : "inactive"}`}>
      <AppIcon name={name} seed={String(session.pid)} exePath={owner?.exePath || undefined} />
      <span className="mix-name">
        <span className="mix-label">{name}</span>
        {!session.active && <Pill>{t("label.standby")}</Pill>}
      </span>
      <Fader
        value={shown}
        peak={session.peak}
        label={`${t("sound.master")} ${name}`}
        hold={hold}
        onInput={(next) => {
          setLocal(next);
          send(next);
        }}
      />
      <Hint label={session.muted ? t("sound.muted") : t("sound.unmute")}>
        <Button
          variant="mute"
          on={session.muted}
          aria-label={session.muted ? t("sound.muted") : t("sound.unmute")}
          onClick={() => {
            void ipc.audioSetVolume(session.role, session.id, session.pid, session.volume, !session.muted);
          }}
        >
          {session.muted ? "⊘" : "♪"}
        </Button>
      </Hint>
      <span className="mix-value num">{Math.round(shown * 100)}%</span>
    </div>
  );
}
