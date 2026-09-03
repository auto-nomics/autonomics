/**
 * Bundled CSL styles — 5 styles shipped with the package (~140KB total).
 *
 * Loaded via Vite's `?raw` suffix so they're inlined as strings at build time.
 * This trades bundle size (~140KB) for offline availability and zero network
 * round-trips on first citation. See plan §9.1 for the bundle-vs-fetch decision.
 *
 * To add a new style:
 *   1. Drop the `.csl` file into `assets/styles/`
 *   2. Add an `import` line below + an entry to `BUNDLED_STYLES`
 */

import apa from '../assets/styles/apa.csl?raw';
import ama from '../assets/styles/american-medical-association.csl?raw';
import ieee from '../assets/styles/ieee.csl?raw';
import nature from '../assets/styles/nature.csl?raw';
import gbT7714 from '../assets/styles/gb-t-7714-2015-numeric.csl?raw';

export interface BundledStyle {
    /** Stable id matching the backend `CslStyle.id` (P1 migration seeds). */
    id: string;
    /** Human-readable name for UI display. */
    title: string;
    /** Locale this style is designed for; drives which `locales-*.xml` to load. */
    locale: string;
    /** The CSL XML source, inlined at build time. */
    cslXml: string;
    /** Tags for grouping in UI (e.g. numeric vs author-date). */
    categories: string[];
}

/**
 * The 5 styles shipped with @autonomics/citation-engine. Id matches the backend
 * `CslStyle.id` so a style selected in Settings can be looked up locally
 * without a network round-trip.
 */
export const BUNDLED_STYLES: readonly BundledStyle[] = [
    {
        id: 'gb-t-7714-2015-numeric',
        title: 'China National Standard GB/T 7714-2015 (numeric)',
        locale: 'zh-CN',
        cslXml: gbT7714,
        categories: ['numeric', 'generic-base', 'chinese'],
    },
    {
        id: 'ieee',
        title: 'IEEE',
        locale: 'en-US',
        cslXml: ieee,
        categories: ['numeric', 'engineering'],
    },
    {
        id: 'apa',
        title: 'American Psychological Association 7th edition',
        locale: 'en-US',
        cslXml: apa,
        categories: ['author-date', 'psychology', 'social-science'],
    },
    {
        id: 'american-medical-association',
        title: 'American Medical Association 11th edition',
        locale: 'en-US',
        cslXml: ama,
        categories: ['numeric', 'medicine'],
    },
    {
        id: 'nature',
        title: 'Nature',
        locale: 'en-US',
        cslXml: nature,
        categories: ['numeric', 'science'],
    },
];

/** Default style when no user preference is set (matches backend P1 seed). */
export const DEFAULT_STYLE_ID = 'gb-t-7714-2015-numeric';

/** Look up a bundled style by id; throws if not found. */
export function getBundledStyle(styleId: string): BundledStyle {
    const style = BUNDLED_STYLES.find(s => s.id === styleId);
    if (!style) {
        throw new Error(
            `@autonomics/citation-engine: style "${styleId}" is not bundled. ` +
                `Bundled: ${BUNDLED_STYLES.map(s => s.id).join(', ')}.`,
        );
    }
    return style;
}
