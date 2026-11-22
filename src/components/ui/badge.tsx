import { type HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * shadcn's Badge shape, which is what the app called a Pill. The variants are the three
 * states it was used in.
 */
const badgeVariants = cva(
  "inline-flex items-center gap-[5px] rounded-full px-[7px] py-0.5 text-[10.5px] font-semibold leading-4",
  {
    variants: {
      variant: {
        default: "bg-surface-3 text-ink-2",
        accent: "bg-accent-tint text-accent",
        faint: "bg-surface-3 text-ink-3",
        danger: "bg-danger-tint text-destructive",
        warn: "bg-warn-tint text-[var(--m-warn)]",
        note: "border border-dashed border-current bg-transparent text-ink-2",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

export interface BadgeProps
  extends HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
