import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * The class joiner every shadcn component expects at `@/lib/utils`: conditional classes
 * through clsx, then conflicting Tailwind utilities resolved by tailwind-merge so that a
 * caller passing `p-4` to a component that defaults to `p-6` gets padding-4, not both.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
