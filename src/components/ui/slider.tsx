import * as SliderPrimitive from "@radix-ui/react-slider";
import { cn } from "@/lib/utils";

/**
 * shadcn's Slider over Radix, with one addition the stock component does not have: `overlay`
 * is drawn inside the track, after the range. The mixer puts the live signal level there, on
 * top of the fill, and the track is the only place it can live — it has to be clipped by the
 * same rounded box the fill is, or a loud stream reads as a level running past the thumb.
 *
 * `label` goes on the thumb rather than the root because the thumb is the element that carries
 * `role="slider"`, and so the one that needs a name.
 *
 * What Radix buys over the `<input type="range">` this replaces: pointer capture, so a drag
 * that leaves the window still ends where the hand lets go; a thumb that stays inside the
 * track at both ends instead of hanging off it; and the value being a real number rather than
 * a string parsed back out of the DOM on every move.
 */
export function Slider({
  className,
  label,
  overlay,
  ...props
}: React.ComponentProps<typeof SliderPrimitive.Root> & {
  label?: string;
  overlay?: React.ReactNode;
}) {
  return (
    <SliderPrimitive.Root
      className={cn(
        // 18px rather than the thumb's 13px: the whole strip is the hit area, and a control
        // that only answers on its own thumb is harder to grab than it looks.
        "relative flex h-[18px] w-full min-w-0 touch-none select-none items-center",
        className,
      )}
      {...props}
    >
      <SliderPrimitive.Track className="relative h-1.5 w-full grow overflow-hidden rounded-full bg-surface-3">
        <SliderPrimitive.Range
          // Radix positions the range with `left`/`right` rather than a transform, so the ease
          // that used to sit on the fill has to name those properties.
          className="absolute h-full w-full bg-accent-fill [transition:left_var(--t-med),right_var(--t-med)]"
        />
        {overlay}
      </SliderPrimitive.Track>
      <SliderPrimitive.Thumb
        aria-label={label}
        className={cn(
          "block size-[13px] rounded-full border border-input bg-surface",
          "[transition:transform_var(--t-fast)] hover:scale-[1.12] active:scale-[1.2]",
          "focus-visible:outline-none focus-visible:border-accent focus-visible:ring-[3px] focus-visible:ring-ring/30",
        )}
      />
    </SliderPrimitive.Root>
  );
}
