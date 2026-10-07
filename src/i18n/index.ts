import { createContext, useContext } from "react";
import en, { type Key } from "./en";
import zh from "./zh-Hans";

export type Lang = "en" | "zh-Hans";

const tables: Record<Lang, Record<Key, string>> = { en, "zh-Hans": zh };

export function systemLang(): Lang {
  const nav = navigator.language ?? "en";
  return nav.toLowerCase().startsWith("zh") ? "zh-Hans" : "en";
}

export function resolveLang(pref: "auto" | Lang): Lang {
  return pref === "auto" ? systemLang() : pref;
}

type Params = Record<string, string | number>;

function lookup(lang: Lang, key: Key, params?: Params): string {
  let out: string = tables[lang][key] ?? tables.en[key] ?? String(key);
  if (!params) return out;
  out = out.replace(/\{(\w+)(?:,\s*plural,\s*(.+))?\}/g, (_m, name: string, forms?: string) => {
    const raw = params[name];
    if (!forms) return raw === undefined ? "" : String(raw);
    const n = typeof raw === "number" ? raw : Number(raw);
    const map: Record<string, string> = {};
    for (const part of forms.split(/\s+/)) {
      const eq = part.indexOf(" ");
      if (eq === -1) continue;
      map[part.slice(0, eq)] = part.slice(eq + 1, -1).replace(/^['"]|['"]$/g, "");
    }
    const pick = lang === "en" && n === 1 ? "one" : "other";
    const form = map[pick] ?? map.other ?? "";
    return form.replace(/#/g, String(raw ?? ""));
  });
  return out;
}

export type Translate = (key: Key, params?: Params) => string;

export function makeT(lang: Lang): Translate {
  return (key, params) => lookup(lang, key, params);
}

export const I18nContext = createContext<{ lang: Lang; t: Translate }>({
  lang: "en",
  t: (key, params) => lookup("en", key, params),
});

export function useI18n() {
  return useContext(I18nContext);
}
