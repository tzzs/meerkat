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

/**
 * `{name}` or `{name, plural, one {# thing} other {# things}}`. The forms are matched as
 * `word {text}` pairs: splitting them on whitespace, as this used to, broke every form that
 * contained a space, which is all of them, and every plural in the app rendered as nothing.
 */
const PLACEHOLDER = /\{(\w+)(?:,\s*plural,\s*((?:[^{}]|\{[^{}]*\})*))?\}/g;
const FORM = /(\w+)\s*\{([^{}]*)\}/g;

function lookup(lang: Lang, key: Key, params?: Params): string {
  const out: string = tables[lang][key] ?? tables.en[key] ?? String(key);
  if (!params) return out;
  return out.replace(PLACEHOLDER, (_m, name: string, forms?: string) => {
    const raw = params[name];
    if (forms === undefined) return raw === undefined ? "" : String(raw);
    const n = typeof raw === "number" ? raw : Number(raw);
    const map: Record<string, string> = {};
    for (const [, form, text] of forms.matchAll(FORM)) map[form] = text;
    const pick = lang === "en" && n === 1 ? "one" : "other";
    return (map[pick] ?? map.other ?? "").replace(/#/g, String(raw ?? ""));
  });
}

/** The label each window tab carries. The power tab's real label depends on the hardware. */
export const TAB_LABEL: Record<string, Key> = {
  overview: "tab.overview",
  history: "tab.history",
  cpu: "tab.cpu",
  memory: "tab.memory",
  disk: "tab.disk",
  network: "tab.network",
  gpu: "tab.gpu",
  battery: "tab.battery",
  sound: "tab.sound",
};

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
