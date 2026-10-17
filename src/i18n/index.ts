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

export type Params = Record<string, string | number>;

/** The index of the brace that closes the one at `open`, or -1. */
function closing(text: string, open: number): number {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === "{") depth++;
    else if (text[i] === "}" && --depth === 0) return i;
  }
  return -1;
}

/** `one {# process} other {# processes}` as selector → text. */
function pluralForms(text: string): Map<string, string> {
  const forms = new Map<string, string>();
  let i = 0;
  const skipSpace = () => {
    while (i < text.length && /\s/.test(text[i])) i++;
  };
  while (i < text.length) {
    skipSpace();
    const start = i;
    while (i < text.length && !/[\s{]/.test(text[i])) i++;
    const selector = text.slice(start, i);
    skipSpace();
    if (!selector || text[i] !== "{") break;
    const end = closing(text, i);
    if (end === -1) break;
    forms.set(selector, text.slice(i + 1, end));
    i = end + 1;
  }
  return forms;
}

function placeholder(lang: Lang, body: string, params: Params): string {
  const match = /^\s*(\w+)\s*(?:,\s*plural\s*,([\s\S]*))?$/.exec(body);
  if (!match) return `{${body}}`;
  const [, name, forms] = match;
  const raw = params[name];
  if (forms === undefined) return raw === undefined ? "" : String(raw);
  const n = typeof raw === "number" ? raw : Number(raw);
  const options = pluralForms(forms);
  const category = Number.isFinite(n) ? new Intl.PluralRules(lang).select(n) : "other";
  const chosen = options.get(`=${n}`) ?? options.get(category) ?? options.get("other") ?? "";
  return format(lang, chosen.replace(/#/g, String(raw ?? "")), params);
}

/**
 * Fills `{name}` and `{name, plural, one {…} other {…}}`. The plural branch used to split the
 * forms on whitespace and then look for a space inside each piece, which never exists, so every
 * plural in the interface rendered as an empty string: the quit dialog said " will be asked to
 * close", the core grid "5% average across ", and "last seen" had no time after it.
 */
export function format(lang: Lang, template: string, params: Params): string {
  let out = "";
  let i = 0;
  while (i < template.length) {
    const open = template.indexOf("{", i);
    if (open === -1) return out + template.slice(i);
    const end = closing(template, open);
    if (end === -1) return out + template.slice(i);
    out += template.slice(i, open) + placeholder(lang, template.slice(open + 1, end), params);
    i = end + 1;
  }
  return out;
}

/**
 * `{name}` or `{name, plural, one {# thing} other {# things}}`. The forms are matched as
 * `word {text}` pairs: splitting them on whitespace, as this used to, broke every form that
 * contained a space, which is all of them, and every plural in the app rendered as nothing.
 */
function lookup(lang: Lang, key: Key, params?: Params): string {
  const template: string = tables[lang][key] ?? tables.en[key] ?? String(key);
  return params ? format(lang, template, params) : template;
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
