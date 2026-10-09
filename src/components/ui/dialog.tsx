import type { ReactNode } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { cn } from "@/lib/utils";

/**
 * shadcn's Dialog, which is a thin skin over Radix's. The reason to take it is not the
 * markup — it is the behaviour that markup alone does not give: focus is moved in and
 * restored on close, Escape and the backdrop dismiss, the rest of the page is inert while
 * it is up, and the exit animation actually plays (Radix keeps the node mounted until the
 * animation ends, which is what the hand-rolled presence hook existed to do).
 */
export function Dialog({ open, onOpenChange, children }: { open: boolean; onOpenChange: (open: boolean) => void; children: ReactNode }) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      {children}
    </DialogPrimitive.Root>
  );
}

export function DialogContent({
  className,
  children,
  label,
}: {
  className?: string;
  children: ReactNode;
  /** Read out as the dialog's name; Radix would otherwise want a visible title element. */
  label: string;
}) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay
        className="fixed inset-0 z-40 bg-overlay data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0"
      />
      <DialogPrimitive.Content
        aria-label={label}
        className={cn(
          "fixed left-1/2 top-1/2 z-50 w-[min(420px,88vw)] -translate-x-1/2 -translate-y-1/2",
          "rounded-md border border-input bg-card p-[22px] text-card-foreground shadow-[var(--shadow)]",
          "data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-bottom-1 data-[state=closed]:zoom-out-95",
          "data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:slide-in-from-bottom-1 data-[state=open]:zoom-in-95",
          className,
        )}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

export function DialogHeader({ className, children }: { className?: string; children: ReactNode }) {
  return <header className={cn("mb-1.5", className)}>{children}</header>;
}

export function DialogTitle({ children }: { children: ReactNode }) {
  return (
    <DialogPrimitive.Title className="m-0 mb-1.5 font-display text-base font-[650] tracking-[-0.01em]">
      {children}
    </DialogPrimitive.Title>
  );
}

export function DialogDescription({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <DialogPrimitive.Description className={cn("m-0 text-[12.5px] leading-normal", className)}>
      {children}
    </DialogPrimitive.Description>
  );
}

export function DialogActions({ children }: { children: ReactNode }) {
  return <div className="mt-4 flex items-center gap-2">{children}</div>;
}

/** Closes the dialog without deciding anything, which is what this button always meant. */
export function DialogClose({ children }: { children: ReactNode }) {
  return <DialogPrimitive.Close asChild>{children}</DialogPrimitive.Close>;
}
