import * as TabsPrimitive from "@radix-ui/react-tabs";
import { cn } from "@/lib/utils";

/**
 * shadcn's Tabs over Radix. What Radix buys here is the part that was missing by hand:
 * arrow keys move between tabs, Home/End jump to the ends, focus is kept on the tablist
 * while navigating, and the roles are correct without anyone remembering to write them.
 *
 * The underline is drawn by CSS on `[role="tab"]::after` (base.css): it is a visual detail of
 * this tab bar rather than of tabs in general, and keying it on data-state keeps it working
 * whichever component owns the trigger.
 */
export const Tabs = TabsPrimitive.Root;

export function TabsList({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      className={cn("flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto", className)}
      {...props}
    />
  );
}

export function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      className={cn(
        "relative whitespace-nowrap rounded-sm px-3 py-1.5 text-[12.5px] font-medium text-ink-2",
        "transition-[background-color,color] hover:bg-surface-2 hover:text-foreground",
        "data-[state=active]:bg-surface-3 data-[state=active]:font-semibold data-[state=active]:text-foreground",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
      {...props}
    />
  );
}

/**
 * No display override here, deliberately: Radix hides an inactive panel with the `hidden`
 * attribute, and any author `display` (including `contents`, which this file first tried)
 * outranks the UA rule for `[hidden]` and leaves every panel on screen at once.
 */
export function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return <TabsPrimitive.Content className={cn("focus-visible:outline-none", className)} {...props} />;
}
