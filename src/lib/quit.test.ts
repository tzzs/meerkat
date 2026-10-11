import { describe, expect, it } from "vitest";
import { makeT } from "../i18n";
import { quitMessageKey } from "./quit";

const result = (over: { asked?: number; forceUsed?: boolean; error?: string | null }) => ({
  asked: 0,
  forceUsed: false,
  error: null,
  ...over,
});

describe("quitMessageKey", () => {
  it("counts the windows that were asked to close", () => {
    expect(quitMessageKey(result({ asked: 2 }))).toEqual({ key: "quit.asked", tone: "ok", count: 2, offerForce: false });
  });

  it("counts the processes a force quit ended", () => {
    expect(quitMessageKey(result({ asked: 5, forceUsed: true })).key).toBe("quit.forced");
  });

  // Every background app has no window, and "Quit" on one used to do nothing visible at all.
  it("offers force quit when there was no window to ask", () => {
    const outcome = quitMessageKey(result({ error: "no-window" }));
    expect(outcome.key).toBe("quit.noWindow");
    expect(outcome.offerForce).toBe(true);
  });

  it("maps each refusal to its own message", () => {
    expect(quitMessageKey(result({ error: "protected" })).key).toBe("quit.protected");
    expect(quitMessageKey(result({ error: "denied" })).key).toBe("quit.denied");
    expect(quitMessageKey(result({ error: "gone" })).key).toBe("quit.gone");
    expect(quitMessageKey(result({ error: "no-reading" })).key).toBe("quit.noReading");
    expect(quitMessageKey(result({ error: "not-an-application" })).key).toBe("quit.notAnApp");
  });

  it("never passes on a code it does not know, or a sentence from the backend", () => {
    expect(quitMessageKey(result({ error: "no window to close; use force quit" })).key).toBe("quit.failed");
    expect(quitMessageKey(result({ error: "eperm" })).offerForce).toBe(false);
  });

  it("reports a call that never answered as a failure", () => {
    expect(quitMessageKey(null).key).toBe("quit.failed");
    expect(quitMessageKey(result({ asked: 0 })).key).toBe("quit.failed");
  });

  it("has words for every outcome in both languages", () => {
    const outcomes = [null, "no-window", "protected", "denied", "gone", "no-reading", "not-an-application", "x"]
      .map((error) => (error === null ? quitMessageKey(result({ asked: 1 })) : quitMessageKey(result({ error }))))
      .concat(quitMessageKey(result({ asked: 3, forceUsed: true })));
    for (const lang of ["en", "zh-Hans"] as const) {
      const t = makeT(lang);
      for (const outcome of outcomes) {
        const text = t(outcome.key, { name: "Notepad", count: outcome.count });
        expect(text).not.toBe(outcome.key);
        expect(text).not.toMatch(/[{}]/);
      }
    }
  });
});
