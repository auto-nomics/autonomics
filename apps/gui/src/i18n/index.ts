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
import zhAgent from "./locales/zh/agent.json";
import enAgent from "./locales/en/agent.json";
import zhBibliography from "./locales/zh/bibliography.json";
import enBibliography from "./locales/en/bibliography.json";
import zhCanvas from "./locales/zh/canvas.json";
import enCanvas from "./locales/en/canvas.json";
import zhDeck from "./locales/zh/deck.json";
import enDeck from "./locales/en/deck.json";
import zhDiscovery from "./locales/zh/discovery.json";
import enDiscovery from "./locales/en/discovery.json";
import zhReader from "./locales/zh/reader.json";
import enReader from "./locales/en/reader.json";
import zhWiki from "./locales/zh/wiki.json";
import enWiki from "./locales/en/wiki.json";

export const defaultNS = "common";
export const resources = {
  zh: {
    common: zhCommon,
    agent: zhAgent,
    bibliography: zhBibliography,
    canvas: zhCanvas,
    deck: zhDeck,
    discovery: zhDiscovery,
    reader: zhReader,
    wiki: zhWiki,
  },
  en: {
    common: enCommon,
    agent: enAgent,
    bibliography: enBibliography,
    canvas: enCanvas,
    deck: enDeck,
    discovery: enDiscovery,
    reader: enReader,
    wiki: enWiki,
  },
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

// Default export for non-React modules translating at the point a string is
// produced: `import i18n from "@/i18n"; i18n.t("ns:key")`.
export default i18next;
