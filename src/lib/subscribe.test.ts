import { describe, expect, it, vi } from "vitest";
import { subscribe } from "./subscribe";

const deferred = () => {
  let resolve!: (unlisten: () => void) => void;
  const promise = new Promise<() => void>((r) => (resolve = r));
  return { promise, resolve };
};
const settle = () => new Promise((r) => setTimeout(r, 0));

describe("subscribe", () => {
  // StrictMode unmounts before `listen` has resolved; that listener must still be removed.
  it("removes a listener whose stop came before it finished registering", async () => {
    const pending = deferred();
    const unlisten = vi.fn();
    const stop = subscribe(() => pending.promise);
    stop();
    pending.resolve(unlisten);
    await settle();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("removes a registered listener when stopped", async () => {
    const unlisten = vi.fn();
    const stop = subscribe(async () => unlisten);
    await settle();
    expect(unlisten).not.toHaveBeenCalled();
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("removes it only once however often it is stopped", async () => {
    const unlisten = vi.fn();
    const stop = subscribe(async () => unlisten);
    await settle();
    stop();
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("survives a registration that fails", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const stop = subscribe(() => Promise.reject(new Error("no event system")));
    await settle();
    expect(() => stop()).not.toThrow();
    expect(error).toHaveBeenCalled();
    error.mockRestore();
  });
});
