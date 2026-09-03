/**
 * Bundled CSL locales — XML localized term/term-format files.
 *
 * citeproc-js requires a `retrieveLocale(lang)` callback returning the locale
 * XML for the requested language. We bundle the two most-used locales
 * (zh-CN, en-US) and throw for anything else — extend `BUNDLED_LOCALES` if a
 * new locale is needed.
 *
 * Total size ~50KB; inlined via Vite `?raw`.
 */

import enUS from '../assets/locales/locales-en-US.xml?raw';
import zhCN from '../assets/locales/locales-zh-CN.xml?raw';

export interface BundledLocale {
    /** BCP-47 language tag, matches CSL `<locale xml:lang="...">`. */
    lang: string;
    /** The locale XML source. */
    xml: string;
}

/** Locales currently bundled with the engine. */
export const BUNDLED_LOCALES: readonly BundledLocale[] = [
    { lang: 'en-US', xml: enUS },
    { lang: 'zh-CN', xml: zhCN },
];

/**
 * Look up a locale by BCP-47 tag. Falls back to `en-US` if exact match missing
 * (citeproc-js will then use English defaults for any untranslated term).
 *
 * Returns the raw XML string suitable for `sys.retrieveLocale`.
 */
export function getLocaleXml(lang: string): string {
    const exact = BUNDLED_LOCALES.find(l => l.lang === lang);
    if (exact) return exact.xml;
    // Strip region suffix and try language-only match (e.g. "zh-Hant" → "zh")
    const short = lang.split('-')[0];
    const partial = BUNDLED_LOCALES.find(l => l.lang.startsWith(short + '-'));
    if (partial) return partial.xml;
    // Final fallback: en-US (citeproc-js default terms)
    return enUS;
}
