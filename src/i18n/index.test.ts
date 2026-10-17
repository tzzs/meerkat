import { describe, expect, it } from "vitest";
import en from "./en";
import zh from "./zh-Hans";
import { format, makeT } from "./index";

describe("plurals", () => {
  const t = makeT("en");
  const z = makeT("zh-Hans");

  // Every plural used to come out as an empty string.
  it("chooses the English form by count", () => {
    expect(t("label.adapters", { count: 1 })).toBe("1 adapter");
    expect(t("label.adapters", { count: 3 })).toBe("3 adapters");
    expect(t("time.minutesAgo", { count: 4 })).toBe("4 minutes ago");
  });

  it("fills plain placeholders around a plural", () => {
    expect(t("cpu.perCoreAverage", { average: "5%", cores: 20 })).toBe("5% average across 20 logical cores");
    expect(t("action.confirmQuitBody", { count: 1 }).startsWith("1 process will be asked")).toBe(true);
  });

  it("uses the only form Chinese has", () => {
    expect(z("action.confirmQuitBody", { count: 3 }).startsWith("3 个进程")).toBe(true);
  });

  it("prefers an exact match over the category", () => {
    expect(format("en", "{n, plural, =0 {none} one {# item} other {# items}}", { n: 0 })).toBe("none");
    expect(format("en", "{n, plural, =0 {none} one {# item} other {# items}}", { n: 2 })).toBe("2 items");
  });

  it("leaves a missing value empty rather than printing the placeholder", () => {
    expect(format("en", "a{x}b", {})).toBe("ab");
  });
});

describe("the dictionaries", () => {
  // A translation that forgets {duration} renders without a brace and without an error, and
  // simply says less than the English did.
  it("use the same placeholders in both languages", () => {
    // Top-level `{name}` and `{name, plural, ...}` only: the forms inside a plural are text.
    const names = (template: string) => {
      const found = new Set<string>();
      let depth = 0;
      for (let i = 0; i < template.length; i++) {
        if (template[i] === "{") {
          if (depth === 0) found.add(/^\w+/.exec(template.slice(i + 1))?.[0] ?? "");
          depth++;
        } else if (template[i] === "}") depth--;
      }
      return [...found].sort();
    };
    for (const key of Object.keys(en) as (keyof typeof en)[]) {
      expect(names(zh[key]), key).toEqual(names(en[key]));
    }
  });

  // tsc already proves both tables have the same keys; this proves they all render.
  it("fill every placeholder in both languages", () => {
    const params = new Proxy({}, { get: () => 2 }) as Record<string, number>;
    for (const [lang, table] of [["en", en], ["zh-Hans", zh]] as const) {
      for (const [key, template] of Object.entries(table)) {
        const text = format(lang, template, params);
        expect(text, `${lang} ${key}`).not.toMatch(/[{}]/);
        expect(text.trim(), `${lang} ${key}`).not.toBe("");
      }
    }
  });
});
