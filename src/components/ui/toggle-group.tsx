import * as ToggleGroupPrimitive from "@radix-ui/react-toggle-group";
import { Button } from "./button";
import { cn } from "@/lib/utils";

/**
 * shadcn's ToggleGroup over Radix. The exclusive chip rows (range, sort) were buttons wearing
 * `aria-pressed`, which tells a screen reader "each of these is a separate on/off" when the
 * truth is "exactly one of these is chosen"; Radix makes that a radiogroup with roving focus
 * and arrow keys, and marks the chosen one with `data-state="on"`.
 */
export function ToggleGroup({
  className,
  ...props
}: React.ComponentProps<typeof ToggleGroupPrimitive.Root>) {
  // Radix moves the cursor on the arrow keys but leaves the choice where it was. A group that
  // claims role="radiogroup" owes the user the other half — the arrows are how you change a
  // radio — so here the choice follows the cursor: focusing a chip that is not the chosen one
  // clicks it. Only for the single case; a multi-select group is a toolbar, where moving the
  // cursor is not a decision.
  return (
    <ToggleGroupPrimitive.Root
      className={cn("flex flex-wrap items-center gap-[3px]", className)}
      {...props}
      onFocus={(event) => {
        props.onFocus?.(event);
        const target = event.target as HTMLElement;
        if (props.type === "single" && target.dataset.state !== "on") target.click();
      }}
    />
  );
}

/** A chip inside a ToggleGroup: the button's chip look, with the chosen state keyed on Radix. */
export function ToggleChip({
  value,
  className,
  ...props
}: React.ComponentProps<typeof ToggleGroupPrimitive.Item>) {
  return (
    <ToggleGroupPrimitive.Item value={value} asChild>
      <Button
        variant="chip"
        className={cn(
          /* Vitals: the chosen chip takes the tone in front of it, and is not made bolder.
             A grey "selected" reads as disabled; a tinted one reads as chosen. */
          "data-[state=on]:bg-[color-mix(in_srgb,var(--tone,var(--accent))_14%,transparent)] data-[state=on]:font-semibold data-[state=on]:text-[var(--tone,var(--accent))]",
          className,
        )}
        {...props}
      />
    </ToggleGroupPrimitive.Item>
  );
}
