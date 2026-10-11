/** The levels the backend's `log_frontend` accepts. */
export type LogLevel = "error" | "warn" | "info";

/** Long enough for a component stack, short enough that one bad loop cannot flood the log. */
export const MAX_REPORT_CHARS = 4000;

/** Whatever was thrown, as text: an Error with its stack, a string as is, anything else as JSON. */
export function describeError(value: unknown): string {
  if (value instanceof Error) {
    const head = `${value.name}: ${value.message}`;
    const stack = value.stack ?? "";
    // V8 starts the stack with the same "Name: message" line; other engines do not.
    return stack.startsWith(head) ? stack : `${head}\n${stack}`.trim();
  }
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

export function clip(text: string, max = MAX_REPORT_CHARS): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

/**
 * Decides whether a report goes out. An error thrown on every frame would otherwise send sixty
 * reports a second across the bridge into a log file; this lets a burst through, then drops
 * everything until the window has passed, and never sends the same message twice within
 * `repeatMs`.
 */
export function createLimiter({ burst, windowMs, repeatMs }: { burst: number; windowMs: number; repeatMs: number }) {
  let sent: number[] = [];
  const lastSeen = new Map<string, number>();
  return (message: string, now: number): boolean => {
    sent = sent.filter((at) => now - at < windowMs);
    const last = lastSeen.get(message);
    if (last !== undefined && now - last < repeatMs) return false;
    if (sent.length >= burst) return false;
    sent.push(now);
    lastSeen.set(message, now);
    // The dedupe table only needs to remember recent messages.
    if (lastSeen.size > 100) {
      for (const [key, at] of lastSeen) if (now - at >= repeatMs) lastSeen.delete(key);
    }
    return true;
  };
}
