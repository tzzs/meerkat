import { useEffect, useState } from "react";

/**
 * A value read from the backend when `deps` change and again every `everyMs`, for the views that
 * show stored history rather than the live sample: history moves once a minute, so polling it
 * any faster would only repeat the same answer.
 */
export function useLoaded<T>(load: () => Promise<T>, initial: T, deps: unknown[], everyMs = 60_000): T {
  const [value, setValue] = useState<T>(initial);
  useEffect(() => {
    let alive = true;
    const run = () => void load().then((next) => alive && setValue(next));
    run();
    const timer = window.setInterval(run, everyMs);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
    // The loader is a new closure every render; what it reads is what the caller lists in deps.
  }, deps);
  return value;
}
