import * as SelectPrimitive from "@radix-ui/react-select";
import { cn } from "@/lib/utils";

/**
 * shadcn's Select over Radix. The native `<select>` was fine until the app grew a theme of its
 * own: its popup is drawn by the operating system, so a dark window opened a white list. Radix
 * draws the list, which means it takes the app's colours, its radius and its scrollbars.
 *
 * Two departures from the stock component, both because the lists here are short: the content
 * scrolls itself rather than through Radix's own up/down buttons, and the check sits at the end
 * of the row because the label is what you are reading.
 */
export const Select = SelectPrimitive.Root;
export const SelectValue = SelectPrimitive.Value;

export function SelectTrigger({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Trigger>) {
  return (
    <SelectPrimitive.Trigger
      className={cn(
        "flex min-w-[150px] items-center justify-between gap-2 rounded-sm border border-border bg-surface-2 px-2 py-[5px] text-left",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50",
        className,
      )}
      {...props}
    >
      {children}
      <svg
        aria-hidden="true"
        viewBox="0 0 10 6"
        className="size-[10px] shrink-0 text-ink-3"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      >
        <path d="M1 1l4 4 4-4" />
      </svg>
    </SelectPrimitive.Trigger>
  );
}

export function SelectContent({
  className,
  children,
  position = "popper",
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Content>) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        className={cn(
          "relative z-50 max-h-[var(--radix-select-content-available-height)] min-w-[8rem] overflow-y-auto overflow-x-hidden",
          "rounded-md border border-input bg-popover text-popover-foreground shadow-card",
          "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
          "data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95",
          position === "popper" && "data-[side=bottom]:translate-y-1 data-[side=top]:-translate-y-1",
          className,
        )}
        position={position}
        {...props}
      >
        <SelectPrimitive.Viewport className="p-1">{children}</SelectPrimitive.Viewport>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  );
}

export function SelectItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Item>) {
  return (
    <SelectPrimitive.Item
      className={cn(
        "relative flex w-full cursor-default select-none items-center rounded-xs py-1.5 pl-2 pr-7 outline-none",
        "data-[highlighted]:bg-surface-2",
        "data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        className,
      )}
      {...props}
    >
      <SelectPrimitive.ItemText>{children}</SelectPrimitive.ItemText>
      <span className="absolute right-2 flex size-3.5 items-center justify-center">
        <SelectPrimitive.ItemIndicator>
          <svg
            aria-hidden="true"
            viewBox="0 0 10 10"
            className="size-[10px] text-accent"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          >
            <path d="M1.5 5.5l2.5 2.5 4.5-6" />
          </svg>
        </SelectPrimitive.ItemIndicator>
      </span>
    </SelectPrimitive.Item>
  );
}
