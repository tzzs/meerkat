import * as ContextMenuPrimitive from "@radix-ui/react-context-menu";
import { cn } from "@/lib/utils";

/**
 * shadcn's ContextMenu over Radix, drawn like the Select list so the two popups read as one
 * family. Radix is what makes it a menu rather than a floating div: arrow keys move through the
 * items, Escape and a click outside close it, and the menu key opens it on a focused row.
 */
export const ContextMenu = ContextMenuPrimitive.Root;
export const ContextMenuTrigger = ContextMenuPrimitive.Trigger;

export function ContextMenuContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Content>) {
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        className={cn(
          "z-50 min-w-[180px] overflow-hidden rounded-md border border-input bg-popover p-1 text-[12.5px] text-popover-foreground shadow-card",
          "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
          "data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95",
          className,
        )}
        {...props}
      >
        {children}
      </ContextMenuPrimitive.Content>
    </ContextMenuPrimitive.Portal>
  );
}

export function ContextMenuItem({
  className,
  danger,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Item> & { danger?: boolean }) {
  return (
    <ContextMenuPrimitive.Item
      className={cn(
        "relative flex cursor-default select-none items-center gap-2 rounded-xs px-2 py-1.5 outline-none",
        "data-[highlighted]:bg-surface-2",
        "data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        danger && "text-destructive data-[highlighted]:bg-danger-tint",
        className,
      )}
      {...props}
    />
  );
}

export function ContextMenuSeparator({ className }: { className?: string }) {
  return <ContextMenuPrimitive.Separator className={cn("-mx-1 my-1 h-px bg-border", className)} />;
}
