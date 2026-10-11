import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { canQuit, processName } from "../lib/apps";
import { useStore } from "../lib/store";
import type { AppInfo } from "../lib/types";

/** Which row the menu is for, and where it opens. */
export interface AppMenuTarget {
  app: AppInfo;
  /** The name as the row shows it, which is what "Copy name" copies. */
  label: string;
  x: number;
  y: number;
  /** Focus goes back here when the menu closes. */
  origin: HTMLElement | null;
}

/**
 * Where a menu opened from `event` belongs. A right click opens at the pointer; the ContextMenu
 * key and Shift+F10 arrive with no pointer position worth using (Chromium reports button 0 for
 * them), so those open under the row's name instead.
 */
export function menuTarget(
  app: AppInfo,
  label: string,
  element: HTMLElement,
  event?: MouseEvent<HTMLElement>,
): AppMenuTarget {
  if (event && event.button === 2 && (event.clientX !== 0 || event.clientY !== 0)) {
    return { app, label, x: event.clientX, y: event.clientY, origin: element };
  }
  const box = element.getBoundingClientRect();
  return { app, label, x: box.left + 32, y: box.bottom - 4, origin: element };
}

/** Whether a key press on a row asks for its menu. */
export function isMenuKey(event: KeyboardEvent<HTMLElement>): boolean {
  return event.key === "ContextMenu" || (event.shiftKey && event.key === "F10");
}

async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // The clipboard refuses a window without focus; there is nothing useful to do about it.
  }
}

interface Item {
  id: string;
  label: string;
  run: () => void;
  danger?: boolean;
}

/**
 * The row menu: pin, the three copies, and Quit… for anything Pulse would offer to close. It
 * replaces a right click that silently toggled the pin, which nobody could discover and which
 * did nothing visible on a row that was already near the top.
 *
 * A plain menu over a portal rather than a dependency: arrow keys, Home/End, Enter/Space via the
 * buttons themselves, Escape and Tab close it, a click anywhere else or the window losing focus
 * dismisses it, and focus returns to the row it came from.
 */
export function AppMenu({
  target,
  pinned,
  onTogglePin,
  onClose,
}: {
  target: AppMenuTarget;
  pinned: boolean;
  onTogglePin: (key: string) => void;
  onClose: () => void;
}) {
  const { t, requestQuit } = useStore();
  const ref = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState({ left: target.x, top: target.y });
  const { app, label } = target;

  const close = (restoreFocus: boolean) => {
    onClose();
    if (restoreFocus && target.origin?.isConnected) target.origin.focus();
  };

  // Measured before paint, so a menu opened near an edge never flashes past it.
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    const box = menu.getBoundingClientRect();
    setPlace({
      left: Math.max(4, Math.min(target.x, window.innerWidth - box.width - 4)),
      top: Math.max(4, Math.min(target.y, window.innerHeight - box.height - 4)),
    });
    menu.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [target]);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    const dismiss = () => onClose();
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", dismiss, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
    };
  }, [onClose]);

  const items: Item[] = [
    { id: "pin", label: pinned ? t("action.unpin") : t("action.pin"), run: () => onTogglePin(app.key) },
    { id: "copyName", label: t("action.copyName"), run: () => void copy(label) },
  ];
  if (app.exePath) {
    items.push(
      { id: "copyProcessName", label: t("action.copyProcessName"), run: () => void copy(processName(app)) },
      { id: "copyProcessPath", label: t("action.copyProcessPath"), run: () => void copy(app.exePath) },
    );
  }
  if (canQuit(app)) items.push({ id: "quit", label: t("action.quitMenu"), run: () => requestQuit(app), danger: true });

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const buttons = [...(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLElement);
    const focus = (index: number) => buttons[(index + buttons.length) % buttons.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        focus(at + 1);
        break;
      case "ArrowUp":
        focus(at - 1);
        break;
      case "Home":
        focus(0);
        break;
      case "End":
        focus(buttons.length - 1);
        break;
      case "Escape":
      case "Tab":
        close(true);
        break;
      default:
        return;
    }
    // Handled here: the panel's own Escape (hide the whole panel) checks for this.
    event.preventDefault();
    event.stopPropagation();
  };

  return createPortal(
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      style={{ left: place.left, top: place.top }}
      onKeyDown={onKeyDown}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item) => (
        <button
          key={item.id}
          role="menuitem"
          tabIndex={-1}
          className={`menu-item${item.danger ? " danger" : ""}`}
          onClick={() => {
            // Quit hands focus to its dialog; everything else gives it back to the row.
            close(item.id !== "quit");
            item.run();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}
