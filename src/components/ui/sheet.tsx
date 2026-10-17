import type { ReactNode } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { cn } from "@/lib/utils";

/**
 * shadcn's Sheet: the Dialog primitive pinned to the window's right edge. It is a dialog for the
 * same reasons the quit confirmation is one: focus moves in and comes back, Escape and the
 * backdrop close it, and the page behind does not take the keyboard while it is up.
 */
export function Sheet({ open, onOpenChange, children }: { open: boolean; onOpenChange: (open: boolean) => void; children: ReactNode }) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      {children}
    </DialogPrimitive.Root>
  );
}

export function SheetContent({ label, className, children }: { label: string; className?: string; children: ReactNode }) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-overlay data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0" />
      <DialogPrimitive.Content
        aria-label={label}
        // The content is live figures, not a form: there is nothing to describe for a reader
        // beyond the title the label already gives it.
        aria-describedby={undefined}
        className={cn(
          "fixed bottom-0 right-0 top-0 z-50 flex w-[min(460px,92vw)] flex-col border-l border-input bg-card text-card-foreground shadow-[var(--shadow)]",
          "data-[state=open]:animate-in data-[state=open]:slide-in-from-right data-[state=open]:duration-200",
          "data-[state=closed]:animate-out data-[state=closed]:slide-out-to-right data-[state=closed]:duration-150",
          className,
        )}
      >
        <DialogPrimitive.Title className="sr-only">{label}</DialogPrimitive.Title>
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

export const SheetClose = DialogPrimitive.Close;
