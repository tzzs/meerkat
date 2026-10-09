import { forwardRef, type ButtonHTMLAttributes } from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * shadcn's Button, with the five families this app actually had as variants instead of five
 * hand-written class names. The values are the ones the stylesheet carried, so the swap is
 * invisible; what changes is that a new control now says `variant="chip"` and gets every
 * state the others have, instead of copying a rule and forgetting a hover.
 *
 * `on` is a separate dimension because a mute is a toggle: it marks a muted channel. Passing
 * it as a prop keeps the state in the markup instead of in string concatenation at the call
 * site. The pickers (range, sort, panel tiles) are not buttons at all any more — they are a
 * ToggleGroup, because "exactly one of these" and "any of these" are not what aria-pressed says.
 *
 * Three affordances deliberately stay in CSS: `.row-quit` (revealed by hovering its row),
 * `.winctl` (the frameless window's own buttons) and `.icon-btn` (the tray panel's pin and
 * close). They are layout-coupled, not general buttons.
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1 whitespace-nowrap select-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        tool: "h-[26px] w-7 rounded-sm text-sm text-ink-2 transition-colors hover:bg-surface-2 hover:text-foreground",
        chip: "rounded-full border border-transparent px-[9px] py-1 text-[11.5px] text-ink-2 transition-[background-color,color,border-color] hover:bg-surface-2",
        primary:
          "self-start rounded-sm bg-primary px-3.5 py-[7px] text-[12.5px] font-semibold text-primary-foreground",
        danger:
          "rounded-sm border border-danger-edge px-3.5 py-[7px] text-[12.5px] font-semibold text-destructive transition-colors hover:bg-danger-tint",
        mute: "h-6 w-6 rounded-xs text-[13px] text-ink-3 transition-colors hover:bg-surface-3 hover:text-foreground",
      },
      on: {
        true: "",
        false: "",
      },
    },
    compoundVariants: [
      {
        variant: "mute",
        on: true,
        className: "bg-danger-tint text-destructive",
      },
    ],
    defaultVariants: { variant: "tool", on: false },
  },
);

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, on, asChild = false, ...props }, ref) => {
    const Component = asChild ? Slot : "button";
    return <Component className={cn(buttonVariants({ variant, on }), className)} ref={ref} {...props} />;
  },
);
Button.displayName = "Button";

export { buttonVariants };
