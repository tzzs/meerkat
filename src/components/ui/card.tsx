import { type HTMLAttributes, type ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * shadcn's Card shape, styled against Pulse's tokens rather than shadcn's palette, so a
 * component copied from the registry can sit next to one we wrote without introducing a
 * second set of colours.
 *
 * The accent bar is a real element, not a `::before` matched on the inline style attribute:
 * the old rule was `.card[style*="--tone"]::before`, which stopped applying the moment
 * anything else wrote to the element's style.
 */

export function Card({
  className,
  tone,
  children,
  ...props
}: HTMLAttributes<HTMLElement> & { tone?: string }) {
  return (
    <section
      className={cn(
        "relative min-w-0 overflow-hidden rounded-md border border-border bg-card p-4 text-card-foreground",
        className,
      )}
      {...props}
    >
      {tone && (
        <span
          aria-hidden="true"
          className="absolute inset-x-0 top-0 h-0.5 opacity-55"
          style={{ background: tone }}
        />
      )}
      {children}
    </section>
  );
}

export function CardHeader({ className, ...props }: HTMLAttributes<HTMLElement>) {
  return <header className={cn("mb-3 flex items-center gap-3", className)} {...props} />;
}

export function CardTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return (
    <h3
      className={cn(
        "m-0 text-xs font-[650] uppercase tracking-[0.03em] text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** What shadcn calls CardAction: the controls that sit at the far end of the header. */
export function CardTools({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={cn("ml-auto flex items-center gap-2", className)}>{children}</div>;
}
