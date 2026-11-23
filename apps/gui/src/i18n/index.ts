// i18n bootstrap for the GUI. Catalogs live in src/i18n/locales/<lang>/<ns>.json
// and are statically imported so the gateway-embedded build stays self-contained
// (no runtime fetches). Language resolution order:
//   1. localStorage("autonomics.lang")
//   2. navigator.language (zh* → zh, otherwise en)
//   3. fallback "zh"
import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { LANGUAGE_STORAGE_KEY, type Language, languages, resolveLanguage } from "./languages";

import zhCommon from "./locales/zh/common.json";
import enCommon from "./locales/en/common.json";

export const defaultNS = "common";
export const resources = {
  zh: { common: zhCommon },
  en: { common: enCommon },
} as const;

void i18next.use(initReactI18next).init({
  lng: resolveLanguage(),
  fallbackLng: "zh",
  defaultNS,
  resources,
  interpolation: { escapeValue: false },
  returnEmptyString: false,
});

export { LANGUAGE_STORAGE_KEY, languages };
export type { Language };

export function setLanguage(lang: Language): void {
  void i18next.changeLanguage(lang);
  try {
    localStorage.setItem(LANGUAGE_STORAGE_KEY, lang);
  } catch {
    // storage unavailable (private mode etc.) — language still applies for this session
  }
  document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
}

document.documentElement.lang = i18next.language === "en" ? "en" : "zh-CN";
