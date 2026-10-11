import { createContext, useContext, useEffect, useState } from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { cn } from "@/lib/utils";

/**
 * shadcn's Tooltip over Radix. The `title` attribute it replaces was not wrong — it was the
 * only part of the app still drawn by Windows: a white box in a dark window, on the OS's
 * schedule rather than the app's, with no way to say "these sixty cells are one sweep, answer
 * at once" and "this row of window titles is not urgent, wait".
 */
export const TooltipProvider = TooltipPrimitive.Provider;
export const Tooltip = TooltipPrimitive.Root;
export const TooltipTrigger = TooltipPrimitive.Trigger;

export function TooltipContent({
  className,
  sideOffset = 6,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        className={cn(
          "z-50 overflow-hidden rounded-sm border border-input bg-popover px-2 py-1 text-[11.5px] text-popover-foreground shadow-card",
          "data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-95",
          className,
        )}
        sideOffset={sideOffset}
        {...props}
      />
    </TooltipPrimitive.Portal>
  );
}

/**
 * Changing this number closes every Hint below it. The tray panel is hidden rather than
 * unmounted, so a tooltip that was up when it lost focus used to be the first thing on screen
 * the next time it opened, pointing at a row that had since moved.
 */
export const TooltipReset = createContext(0);

/** A tooltip around a control that already exists and already knows how to be clicked. */
export function Hint({
  label,
  delayDuration,
  className,
  children,
}: {
  label: React.ReactNode;
  delayDuration?: number;
  className?: string;
  children: React.ReactElement;
}) {
  const reset = useContext(TooltipReset);
  const [open, setOpen] = useState(false);
  useEffect(() => setOpen(false), [reset]);
  return (
    <Tooltip open={open} onOpenChange={setOpen} delayDuration={delayDuration}>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className={className}>{label}</TooltipContent>
    </Tooltip>
  );
}
