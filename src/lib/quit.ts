import type { Key } from "../i18n/en";
import type { QuitResult } from "./types";

export type ToastTone = "ok" | "warn" | "danger";

export interface QuitOutcome {
  key: Key;
  tone: ToastTone;
  /** Shown as {count}: windows asked, or processes ended. */
  count: number;
  /** "No window to close": the only way left is force quit, so the notice offers it. */
  offerForce: boolean;
}

/**
 * What to tell the user after a quit. The answer used to be thrown away, so an app without a
 * visible window - every background app - ignored "Quit" in silence. The backend reports a
 * code, never a sentence, and every code maps to a translated message here; one this build
 * does not know gets the generic failure rather than raw text.
 */
export function quitMessageKey(result: QuitResult | null): QuitOutcome {
  const outcome = (key: Key, tone: ToastTone, offerForce = false): QuitOutcome => ({
    key,
    tone,
    count: result?.asked ?? 0,
    offerForce,
  });
  if (!result) return outcome("quit.failed", "danger");
  switch (result.error) {
    case null:
      if (result.asked > 0) return outcome(result.forceUsed ? "quit.forced" : "quit.asked", "ok");
      return outcome("quit.failed", "danger");
    case "no-window":
      return outcome("quit.noWindow", "warn", true);
    case "protected":
      return outcome("quit.protected", "danger");
    case "denied":
      return outcome("quit.denied", "danger");
    case "gone":
      return outcome("quit.gone", "warn");
    case "no-reading":
      return outcome("quit.noReading", "warn");
    case "not-an-application":
      return outcome("quit.notAnApp", "warn");
    default:
      return outcome("quit.failed", "danger");
  }
}
