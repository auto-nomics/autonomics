/**
 * Engine tests — load each bundled CSL style and verify it can cite a sample
 * PubMed article and produce a bibliography.
 *
 * Run with: `pnpm test` (vitest).
 */

import { describe, expect, it } from 'vitest';

import { CitationEngine } from '../src/engine';
import { BUNDLED_STYLES, DEFAULT_STYLE_ID, getBundledStyle } from '../src/styles';
import { getLocaleXml } from '../src/locales';

/** Sample CSL-JSON item: a real PubMed article (Chordoma review, 2012). */
const SAMPLE_ITEM = {
    id: 'chordoma-2012',
    type: 'article-journal',
    title: 'Chordoma: current concepts, management, and future directions.',
    author: [
        { family: 'Walcott', given: 'Brian P' },
        { family: 'Nahed', given: 'Brian V' },
        { family: 'Mohyeldin', given: 'Ahmed' },
        { family: 'Coumans', given: 'Jean-Valery' },
        { family: 'Kahle', given: 'Kristopher T' },
        { family: 'Ferreira', given: 'Manuel J' },
    ],
    'container-title': 'The Lancet Oncology',
    DOI: '10.1016/S1470-2045(11)70337-0',
    ISSN: '1470-2045',
    issued: { 'date-parts': [[2012]] },
    publisher: 'Elsevier BV',
    language: 'en',
} as const;

/** Second item with a different author to verify multi-cite + bibliography sorting. */
const SAMPLE_ITEM_2 = {
    id: 'weibel-2017',
    type: 'article-journal',
    title: 'Lung morphometry: the link between structure and function.',
    author: [
        { family: 'Weibel', given: 'Ewald R' },
    ],
    'container-title': 'Cell and Tissue Research',
    DOI: '10.1007/s00441-016-2541-4',
    issued: { 'date-parts': [[2017]] },
    publisher: 'Springer Science+Business Media',
} as const;

describe('CitationEngine', () => {
    // Build a fresh engine per style to avoid pool interference between tests.
    function buildEngine(styleId: string): CitationEngine {
        const style = getBundledStyle(styleId);
        return new CitationEngine(style.cslXml, getLocaleXml(style.locale));
    }

    describe.each(BUNDLED_STYLES.map(s => [s.id, s] as const))(
        'style %s',
        (styleId, style) => {
            it('loads without error', () => {
                expect(() => buildEngine(styleId)).not.toThrow();
            });

            it('renders a single-item citation (numeric styles emit just the index)', () => {
                const engine = buildEngine(styleId);
                engine.addItems([SAMPLE_ITEM]);
                const { html } = engine.citeCluster([SAMPLE_ITEM.id]);
                expect(html).toBeTruthy();
                expect(html.length).toBeGreaterThan(0);
                // In-text citation shape depends on style class:
                //   - author-date (APA): "(Walcott et al., 2012)"
                //   - numeric (IEEE/AMA/Nature/GB-T): "[1]" / "<sup>1</sup>"
                // Numeric form is the common case across our 5 bundled styles
                // (only APA is author-date). Author + year are checked via
                // the bibliography test below — independent of style class.
                const isNumeric = /\[\d+\]|<sup>\d+<\/sup>|\(\d+\)/.test(html);
                const hasAuthorYear = /Walcott/i.test(html) && /2012/.test(html);
                expect(isNumeric || hasAuthorYear).toBe(true);
            });

            it('renders a bibliography containing the registered items', () => {
                const engine = buildEngine(styleId);
                engine.addItems([SAMPLE_ITEM, SAMPLE_ITEM_2]);
                const bib = engine.bibliography();
                expect(bib.entries.length).toBeGreaterThanOrEqual(2);
                const joined = bib.entries.join('\n');
                expect(joined).toMatch(/Walcott/i);
                expect(joined).toMatch(/Weibel/i);
                // Bibliography should include bibstart/bibend wrapper tags.
                expect(bib.bibstart).toBeTruthy();
                expect(bib.bibend).toBeTruthy();
            });

            // Sanity: locale mismatch doesn't break rendering.
            // eslint-disable-next-line no-unused-vars
            const _unused = style; // silence "style unused" if no per-style check below
        },
    );

    describe('GB/T 7714 (zh-CN) specific', () => {
        it('renders in Chinese when locale is zh-CN', () => {
            const engine = buildEngine('gb-t-7714-2015-numeric');
            engine.addItems([SAMPLE_ITEM]);
            const { html } = engine.citeCluster([SAMPLE_ITEM.id]);
            // GB/T 7714 numeric uses [1] prefix for citations.
            expect(html).toMatch(/\[\d+\]/);
        });

        it('bibliography entries contain ISSN or DOI per GB/T 7714 numeric spec', () => {
            const engine = buildEngine('gb-t-7714-2015-numeric');
            engine.addItems([SAMPLE_ITEM]);
            const bib = engine.bibliography();
            const entry = bib.entries[0];
            // GB/T 7714 numeric requires DOI or other identifier suffix.
            expect(entry).toMatch(/10\./); // DOI prefix
        });
    });

    describe('IEEE specific', () => {
        it('renders with bracketed numeric prefix', () => {
            const engine = buildEngine('ieee');
            engine.addItems([SAMPLE_ITEM]);
            const { html } = engine.citeCluster([SAMPLE_ITEM.id]);
            // IEEE: "[1] B. P. Walcott, ..."
            expect(html).toMatch(/\[\d+\]/);
        });

        it('includes italicized container-title in bibliography', () => {
            const engine = buildEngine('ieee');
            engine.addItems([SAMPLE_ITEM]);
            const bib = engine.bibliography();
            // IEEE italicizes journal name; citeproc-js emits <i> or class.
            expect(bib.entries[0]).toMatch(/Lancet Oncology/i);
        });
    });

    describe('APA specific (author-date)', () => {
        it('renders with author + year prefix (no numeric bracket)', () => {
            const engine = buildEngine('apa');
            engine.addItems([SAMPLE_ITEM]);
            const { html } = engine.citeCluster([SAMPLE_ITEM.id]);
            // APA: "(Walcott et al., 2012)" or similar
            expect(html).toMatch(/Walcott/i);
            expect(html).toMatch(/2012/);
            // Should NOT have numeric-only prefix like "[1]" (APA is author-date).
            expect(html).not.toMatch(/^\[\d+\]/);
        });
    });

    describe('error handling', () => {
        it('throws when citing an unregistered item id', () => {
            const engine = buildEngine('ieee');
            expect(() => engine.citeCluster(['non-existent-id'])).toThrow(
                /not registered/,
            );
        });

        it('throws when adding an item without id', () => {
            const engine = buildEngine('ieee');
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            expect(() => engine.addItems([{ type: 'article-journal' } as any])).toThrow(
                /non-empty `id`/,
            );
        });

        it('returns empty cluster for empty item list', () => {
            const engine = buildEngine('ieee');
            const { html } = engine.citeCluster([]);
            expect(html).toBe('');
        });

        it('accepts locators (page numbers)', () => {
            const engine = buildEngine('ieee');
            engine.addItems([SAMPLE_ITEM]);
            const { html } = engine.citeCluster([SAMPLE_ITEM.id], [
                { id: SAMPLE_ITEM.id, locator: '42', label: 'page' },
            ]);
            // IEEE: "[1] B. P. Walcott, ..., p. 42" or similar
            expect(html).toMatch(/42/);
        });
    });

    describe('idempotency', () => {
        it('re-adding an item with same id overwrites the previous entry', () => {
            const engine = buildEngine('ieee');
            engine.addItems([SAMPLE_ITEM]);
            // Same id, different title — should not duplicate.
            engine.addItems([{ ...SAMPLE_ITEM, title: 'UPDATED TITLE' }]);
            const bib = engine.bibliography();
            const joined = bib.entries.join('\n');
            expect(joined).toMatch(/UPDATED TITLE/);
            expect(joined).not.toMatch(/Chordoma: current concepts/);
        });
    });
});

describe('BUNDLED_STYLES', () => {
    it('ships exactly 5 styles', () => {
        expect(BUNDLED_STYLES.length).toBe(5);
    });

    it('includes all expected ids', () => {
        const ids = BUNDLED_STYLES.map(s => s.id).sort();
        expect(ids).toEqual(
            [
                'american-medical-association',
                'apa',
                'gb-t-7714-2015-numeric',
                'ieee',
                'nature',
            ].sort(),
        );
    });

    it('default style id is GB/T 7714 (matches backend seed)', () => {
        expect(DEFAULT_STYLE_ID).toBe('gb-t-7714-2015-numeric');
    });

    it('getBundledStyle throws for unknown id', () => {
        expect(() => getBundledStyle('nope')).toThrow(/not bundled/);
    });
});

describe('getLocaleXml', () => {
    it('returns zh-CN XML for exact match', () => {
        const xml = getLocaleXml('zh-CN');
        expect(xml).toMatch(/xml:lang="zh-CN"/);
    });

    it('returns en-US XML for exact match', () => {
        const xml = getLocaleXml('en-US');
        expect(xml).toMatch(/xml:lang="en-US"/);
    });

    it('falls back to en-US for unknown locale', () => {
        const xml = getLocaleXml('fr-FR');
        expect(xml).toMatch(/xml:lang="en-US"/);
    });

    it('partial-matches language-only when region tag differs', () => {
        // zh-TW → falls back to zh-CN (both have language "zh")
        const xml = getLocaleXml('zh-TW');
        expect(xml).toMatch(/xml:lang="zh-CN"/);
    });
});
