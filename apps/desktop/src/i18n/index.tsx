import { createContext, useContext, useMemo, type ReactNode } from "react";
import type { Language } from "../lib/types";
import { ApiError } from "../lib/api";
import { de, type MessageKey } from "./de";
import { en } from "./en";

export type { MessageKey } from "./de";

const catalogs: Record<Language, Record<MessageKey, string>> = { de, en };

type Vars = Record<string, string | number>;

/** Keys that exist as both `<base>_one` and `<base>_other`. */
export type PluralBase = {
  [K in MessageKey]: K extends `${infer B}_one` ? (`${B}_other` extends MessageKey ? B : never) : never;
}[MessageKey];

export interface I18n {
  lang: Language;
  locale: string;
  t: (key: MessageKey, vars?: Vars) => string;
  /** Plural-aware lookup; `{n}` is available in the message. */
  tp: (base: PluralBase, n: number, vars?: Vars) => string;
  formatDate: (ms: number) => string;
  formatDateTime: (ms: number) => string;
  formatRelative: (ms: number) => string;
  /** Translates any error thrown by the API layer into a user-facing message. */
  errorText: (err: unknown) => string;
}

function interpolate(template: string, vars?: Vars): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = vars[name];
    return value === undefined ? whole : String(value);
  });
}

function createI18n(lang: Language): I18n {
  const messages = catalogs[lang];
  const locale = lang === "de" ? "de-DE" : "en-US";
  const t = (key: MessageKey, vars?: Vars) => interpolate(messages[key], vars);
  const dateFmt = new Intl.DateTimeFormat(locale, { dateStyle: "medium" });
  const dateTimeFmt = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  const relFmt = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });

  const formatRelative = (ms: number) => {
    const diff = ms - Date.now();
    const abs = Math.abs(diff);
    const minute = 60_000;
    const hour = 60 * minute;
    const day = 24 * hour;
    if (abs < minute) return t("time.justNow");
    if (abs < hour) return relFmt.format(Math.round(diff / minute), "minute");
    if (abs < day) return relFmt.format(Math.round(diff / hour), "hour");
    if (abs < 30 * day) return relFmt.format(Math.round(diff / day), "day");
    if (abs < 365 * day) return relFmt.format(Math.round(diff / (30 * day)), "month");
    return relFmt.format(Math.round(diff / (365 * day)), "year");
  };

  const errorText = (err: unknown): string => {
    const apiErr = err instanceof ApiError ? err : null;
    const code = apiErr?.code ?? "unknown";
    const detail = apiErr?.detail ?? (err instanceof Error ? err.message : String(err));
    switch (code) {
      case "wrong_password":
        return t("error.wrong_password");
      case "locked":
        return t("error.locked");
      case "not_found":
        return t("error.not_found");
      case "conflict":
        return t("error.conflict");
      case "invalid_input":
        return t("error.invalid_input", { detail });
      case "io":
        return t("error.io", { detail });
      case "corrupt":
        return t("error.corrupt", { detail });
      case "unsupported":
        return t("error.unsupported", { detail });
      default:
        return detail ? t("error.unknown_detail", { detail }) : t("error.unknown");
    }
  };

  return {
    lang,
    locale,
    t,
    tp: (base, n, vars) => {
      const key = `${base}_${n === 1 ? "one" : "other"}` as MessageKey;
      return interpolate(messages[key], { n, ...vars });
    },
    formatDate: (ms) => dateFmt.format(new Date(ms)),
    formatDateTime: (ms) => dateTimeFmt.format(new Date(ms)),
    formatRelative,
    errorText,
  };
}

const I18nContext = createContext<I18n>(createI18n("de"));

export function I18nProvider({ lang, children }: { lang: Language; children: ReactNode }) {
  const value = useMemo(() => createI18n(lang), [lang]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** Access translations: `const { t } = useT(); t("sidebar.all")`. */
export function useT(): I18n {
  return useContext(I18nContext);
}
