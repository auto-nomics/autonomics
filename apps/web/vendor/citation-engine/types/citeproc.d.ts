/**
 * Minimal ambient declaration for the `citeproc` npm package
 * (a.k.a. citeproc-js, the Zotero citation engine).
 *
 * Source: https://www.npmjs.com/package/citeproc (v2.4.63)
 *
 * The runtime exports a namespace object via `module.exports = CSL`. The
 * namespace itself is *not* a constructor — `new CSL(...)` throws. The actual
 * engine constructor lives at `CSL.Engine(sys, styleXml, lang?)`. This matches
 * the citeproc-js upstream API: https://github.com/Juris-M/citeproc-js
 *
 * This declaration captures only the surface area that @autonomics/citation-engine
 * actually uses. Expand as new features need it.
 */

declare module 'citeproc' {
    /** A CSL-JSON name. See CSL 1.0.1 spec. */
    export interface CslName {
        family?: string;
        given?: string;
        literal?: string;
        'non-dropping-particle'?: string;
        'dropping-particle'?: string;
        suffix?: string;
    }

    /** A CSL-JSON date. `date-parts` is the canonical form. */
    export interface CslDate {
        'date-parts'?: Array<Array<number | string>>;
        season?: string;
        circa?: string;
        literal?: string;
        raw?: string;
    }

    /**
     * A CSL-JSON item. The shape is intentionally permissive — fields vary
     * by item type (article-journal has `container-title`, book doesn't, etc).
     */
    export interface CslItem {
        id: string;
        type: string;
        title?: string;
        author?: CslName[];
        'container-title'?: string;
        'container-title-short'?: string;
        volume?: string | number;
        issue?: string | number;
        page?: string;
        DOI?: string;
        ISBN?: string;
        ISSN?: string;
        PMID?: string;
        URL?: string;
        issued?: CslDate;
        accessed?: CslDate;
        publisher?: string;
        'publisher-place'?: string;
        language?: string;
        abstract?: string;
        note?: string;
        [key: string]: unknown;
    }

    /** Result of `processCitationCluster` — contains rendered string + bib change list. */
    export interface ProcessedCitationCluster {
        bibchange: boolean;
        citation_errors: Array<{ index: number; note: string; citation_id: string }>;
        bibend: number;
        bibstart: number;
        cluster: string;
        engine_update: unknown;
    }

    /**
     * Tuple returned by `CSL.Engine#makeBibliography`. citeproc-js returns
     * `[params, entries]` — an array, not an object. We destructure accordingly.
     */
    export type BibliographyOutput = [BibliographyParams, string[]];

    /** The params half of {@link BibliographyOutput}. */
    export interface BibliographyParams {
        bibstart: string;
        bibend: string;
        entry_ids: string[][];
        maxoffset: number;
        entryspacing: number;
        linespacing: number;
        'second-field-align': string | boolean;
        hangingindent?: number;
        bibliography_errors: unknown[];
        [key: string]: unknown;
    }

    /** Sys object passed to the engine constructor. */
    export interface CiteprocSys {
        retrieveLocale: (lang: string) => string | false;
        retrieveItem: (id: string) => CslItem;
    }

    /** A citation (cluster of items at one position in the document). */
    export interface CiteprocCitation {
        citationID: string;
        citationItems: Array<{
            id: string;
            item?: CslItem;
            prefix?: string;
            suffix?: string;
            locator?: string;
            label?: string;
            'suppress-author'?: boolean;
            'author-only'?: boolean;
            uris?: string[];
        }>;
        properties: {
            noteIndex?: number;
            'in-text'?: boolean;
        };
    }

    /**
     * The citeproc-js engine instance.
     *
     * Constructed via `new CSL.Engine(sys, styleXml, lang?)` — note the
     * `.Engine` accessor on the namespace, not `new CSL(...)`.
     */
    export interface CslEngine {
        /**
         * Process a new citation cluster; returns rendered string + bib deltas.
         * Use this when citations are added incrementally and you want
         * disambiguation / first-reference-subsequent form to update.
         */
        processCitationCluster(
            citation: CiteprocCitation,
            citationsPre: Array<[string, number]>,
            citationsPost: Array<[string, number]>,
        ): ProcessedCitationCluster;

        /**
         * Render a one-off cluster without registering it in the document state.
         * Simpler than `processCitationCluster` but doesn't compute first/subsequent
         * form across multiple calls.
         */
        makeCitationCluster(citationItems: Array<{ id: string; locator?: string; label?: string }>): string;

        /** Render the full bibliography as HTML/string entries. Returns `[params, entries]`. */
        makeBibliography(mode?: 'html' | 'text' | 'rtf'): BibliographyOutput;

        /**
         * Tell the engine which item ids exist (it will call sys.retrieveItem
         * for each). Call this after registering items in the sys layer.
         */
        updateItems(itemIds: string[]): void;
    }

    /** Engine constructor function on the CSL namespace. */
    export type CslEngineConstructor = new (
        sys: CiteprocSys,
        styleXml: string,
        lang?: string,
    ) => CslEngine;

    /**
     * The CSL namespace. The citeproc-js library exports this object as the
     * default export. It is *not* itself a constructor — call `new CSL.Engine(...)`.
     */
    const CSL: {
        Engine: CslEngineConstructor;
        [key: string]: unknown;
    };

    export default CSL;
}
