import { useCallback, useSyncExternalStore } from "react";

/**
 * Pinned apps, by key, in the order they were pinned. They lived in each table's own state,
 * so a pin vanished on the next tab change and never reached the other window or the next
 * launch. localStorage is shared by both webviews (same origin), and its `storage` event tells
 * one window when the other changed it.
 */
export const PINS_KEY = "pulse.pins";

/** Far more than anyone pins; it only bounds what a corrupted value can make the list do. */
export const MAX_PINS = 200;

/** Whatever is stored, as a clean list: anything malformed reads as no pins at all. */
export function parsePins(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return [];
    const keys = value.filter((key): key is string => typeof key === "string" && key.length > 0);
    return [...new Set(keys)].slice(0, MAX_PINS);
  } catch {
    return [];
  }
}

export function togglePin(pins: readonly string[], key: string): string[] {
  return pins.includes(key) ? pins.filter((k) => k !== key) : [...pins, key].slice(-MAX_PINS);
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

let cache: string[] | null = null;
const listeners = new Set<() => void>();
let watching = false;

function current(): string[] {
  if (cache === null) {
    try {
      cache = parsePins(storage()?.getItem(PINS_KEY) ?? null);
    } catch {
      cache = [];
    }
  }
  return cache;
}

function emit() {
  for (const listener of listeners) listener();
}

function watch(listener: () => void): () => void {
  listeners.add(listener);
  if (!watching && typeof window !== "undefined") {
    watching = true;
    // Fired only in the *other* window, which is the one that needs to hear about it.
    window.addEventListener("storage", (event) => {
      if (event.key !== PINS_KEY && event.key !== null) return;
      cache = null;
      emit();
    });
  }
  return () => listeners.delete(listener);
}

export function setPins(next: string[]): void {
  cache = next;
  try {
    storage()?.setItem(PINS_KEY, JSON.stringify(next));
  } catch {
    // Storage refused: the pin still holds for this session in this window.
  }
  emit();
}

/** The pins and a toggle, shared by every list in both windows. */
export function usePins(): [string[], (key: string) => void] {
  const pins = useSyncExternalStore(watch, current, current);
  const toggle = useCallback((key: string) => setPins(togglePin(current(), key)), []);
  return [pins, toggle];
}
