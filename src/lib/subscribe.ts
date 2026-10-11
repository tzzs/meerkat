/**
 * Starts an asynchronous subscription and hands back a stop that works straight away.
 *
 * Tauri's `listen` resolves its unlisten function only after a round trip, and an effect's
 * cleanup can run before that. The old pattern assigned the unlisten function to a variable
 * after the await, so React StrictMode's mount–unmount–mount left the first `metrics` listener
 * registered for the life of the window: every snapshot was then applied twice. Here a stop
 * that arrives early is remembered, and the listener is removed the moment it exists.
 */
export function subscribe(start: () => Promise<() => void>): () => void {
  let stopped = false;
  let stop: (() => void) | null = null;
  void start().then(
    (unlisten) => {
      if (stopped) unlisten();
      else stop = unlisten;
    },
    (error: unknown) => console.error("subscription failed", error),
  );
  return () => {
    if (stopped) return;
    stopped = true;
    stop?.();
    stop = null;
  };
}
