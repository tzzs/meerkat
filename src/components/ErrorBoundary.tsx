import { Component, useState, type ErrorInfo, type ReactNode } from "react";
import type { Translate } from "../i18n";
import { ipc } from "../lib/ipc";
import { clip, createLimiter, describeError, type LogLevel } from "../lib/report";
import { useStore } from "../lib/store";
import { Button } from "./ui/button";

const allow = createLimiter({ burst: 5, windowMs: 10_000, repeatMs: 60_000 });

/** Sends one line to Meerkat's log, unless the limiter has already heard enough of it. */
export function reportFrontend(level: LogLevel, message: string): void {
  const text = clip(message);
  if (!allow(text, Date.now())) return;
  ipc.logFrontend(level, text);
}

/**
 * Errors that never reach a boundary: a throw in an event handler or a timer, and promises
 * nobody awaited. Before this they went to a console no one has open in a tray app.
 */
export function installErrorReporting(where: string): void {
  window.addEventListener("error", (event) => {
    reportFrontend("error", `${where}: ${describeError(event.error ?? event.message)}`);
  });
  window.addEventListener("unhandledrejection", (event) => {
    reportFrontend("error", `${where}: unhandled rejection: ${describeError(event.reason)}`);
  });
}

interface BoundaryProps {
  where: string;
  t: Translate;
  children: ReactNode;
}

interface BoundaryState {
  failed: boolean;
  details: string;
}

/**
 * What a render exception leaves behind. Without it React unmounts the whole tree and the
 * window goes blank with nothing to say why — which is how the tray panel looked for as long
 * as a bad tile id was in the settings file (aff6447).
 */
class ErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false, details: "" };

  static getDerivedStateFromError(error: unknown): Partial<BoundaryState> {
    return { failed: true, details: describeError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    const details = `${describeError(error)}\n${info.componentStack ?? ""}`.trim();
    this.setState({ details });
    reportFrontend("error", `${this.props.where}: render failed: ${details}`);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return <CrashNotice t={this.props.t} details={this.state.details} />;
  }
}

function CrashNotice({ t, details }: { t: Translate; details: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="crash" role="alert">
      <h2 className="crash-title">{t("crash.title")}</h2>
      <p className="crash-body">{t("crash.body")}</p>
      <div className="crash-actions">
        <Button variant="primary" onClick={() => window.location.reload()}>
          {t("crash.reload")}
        </Button>
        <Button
          variant="chip"
          onClick={() => {
            void navigator.clipboard
              .writeText(details)
              .then(() => setCopied(true))
              .catch(() => undefined);
          }}
        >
          {copied ? t("action.copied") : t("crash.copyDetails")}
        </Button>
      </div>
    </div>
  );
}

/** The boundary, inside the providers so that its notice speaks the user's language. */
export function Guarded({ where, children }: { where: string; children: ReactNode }) {
  const { t } = useStore();
  return (
    <ErrorBoundary where={where} t={t}>
      {children}
    </ErrorBoundary>
  );
}
