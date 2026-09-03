/**
 * CitationEngine — thin wrapper around citeproc-js.
 *
 * One CitationEngine instance per CSL style. The engine is *not* shared across
 * styles because citeproc-js bakes the style's macros into its internal state
 * at construction time. Switching styles means rebuilding the engine.
 *
 * Lifecycle:
 *   const engine = new CitationEngine(styleXml, localeXml);
 *   engine.addItems([item1, item2]);           // register CSL-JSON items
 *   const cite = engine.citeCluster(['item1']); // "[1] Author, Title, ..."
 *   const bib = engine.bibliography();           // ['<entry html>', ...]
 *
 * citeproc-js ships as CommonJS (`module.exports = CSL`). The default export is
 * a namespace object (not itself a constructor); the engine constructor lives
 * at `CSL.Engine(sys, styleXml, lang?)` — see upstream manual:
 * https://github.com/Juris-M/citeproc-js/blob/master/manual.md
 */

import CSL from 'citeproc';
import type { CslEngine, CslItem } from 'citeproc';

/** A locator + label pair, e.g. `{ locator: '42', label: 'page' }`. */
export interface CitationClusterLocator {
    /** The CSL item id this locator applies to. */
    id: string;
    /** Locator value (page number, section, etc). */
    locator: string;
    /** CSL locator label: 'page' | 'book' | 'chapter' | 'section' | 'figure' | 'table' | 'note' | 'equation' | ... */
    label?: string;
}

/** Output of `citeCluster` — currently just the rendered string. */
export interface CitationResult {
    /** The rendered citation cluster as HTML (or plain text, depending on style). */
    html: string;
}

/** Output of `bibliography` — list of HTML entries + the engine's bib wrapper tags. */
export interface BibliographyResult {
    /** Opening tag (e.g. `<div class="csl-bib-body">`) — prepend to joined entries. */
    bibstart: string;
    /** Closing tag — append after joined entries. */
    bibend: string;
    /** One HTML string per registered item, in the bibliography's sort order. */
    entries: string[];
}

/**
 * The `params` element of citeproc-js's `[params, entries]` bibliography tuple.
 * We only read `bibstart`/`bibend` from it today; the rest is opaque.
 */
interface BibliographyParams {
    bibstart: string;
    bibend: string;
    entry_ids: string[][];
    [key: string]: unknown;
}

/**
 * CSL item accepted by `addItems`. The shape is CSL-JSON 1.0.x; we accept a
 * permissive type so callers don't have to import the citeproc types just to
 * cite something. The backend `/api/items/:id/csl-json` returns this shape.
 */
export type CiteprocItem = CslItem;

/**
 * citeproc-js engine wrapper. Constructed per style.
 *
 * Note: the constructor is *not* async (the `citeproc` npm package is already
 * loaded synchronously via static `import`). For lazy-loading in the React
 * layer, use `useCitationEngine` which dynamic-imports this module.
 */
export class CitationEngine {
    private readonly items = new Map<string, CslItem>();
    private readonly proc: CslEngine;
    /**
     * 多语言 locale 表。citeproc-js 在引用非构造时 locale 的语言时会
     * 调 retrieveLocale(lang) 拉对应 locale（如英文样式引用中文条目时
     * 可能要 zh-CN locale 做 sort/term）。旧实现忽略 lang 一律返回构造
     * locale, 引文排序 / 术语会错。
     */
    private readonly locales = new Map<string, string>();

    /**
     * @param styleXml The CSL style XML (e.g. IEEE, APA).
     * @param localeXml The CSL locale XML (e.g. locales-en-US.xml). 作为 fallback +
     *                  默认 locale 注册到 `locales` 表中, key 用 "en-US"。
     * @param extraLocales 额外 locale 表 (optional Map<lang, xml>), 用于多语言引用。
     */
    constructor(styleXml: string, localeXml: string, extraLocales?: Map<string, string>) {
        this.locales.set('en-US', localeXml);
        if (extraLocales) {
            for (const [lang, xml] of extraLocales) {
                this.locales.set(lang, xml);
            }
        }

        // `retrieveItem` must be a stable closure that reads from `this.items`.
        // citeproc-js calls it on demand for each citation item.
        const sys = {
            // citeproc-js 传 lang (如 "zh-CN", "fr-FR")。查表, 缺失时回退到 en-US。
            // 旧实现 `_lang: string` 直接忽略, 多语言引用时用错 locale 排序。
            retrieveLocale: (lang: string) => {
                const xml = this.locales.get(lang);
                if (xml) return xml;
                // fallback 链: 同主语言不同地区 ("zh-CN" → "zh")
                const primary = lang.split('-')[0];
                for (const [key, val] of this.locales) {
                    if (key.split('-')[0] === primary) return val;
                }
                // 最终 fallback: en-US (一定有, 构造时塞入)
                return this.locales.get('en-US') ?? localeXml;
            },
            retrieveItem: (id: string) => {
                const item = this.items.get(id);
                if (!item) {
                    throw new Error(
                        `@autonomics/citation-engine: item "${id}" not registered. ` +
                            `Call addItems() before citeCluster() or bibliography().`,
                    );
                }
                return item;
            },
        };
        this.proc = new CSL.Engine(sys, styleXml);
    }

    /**
     * Register CSL-JSON items so they can be cited.
     * Idempotent — re-adding an id overwrites the previous entry.
     */
    addItems(items: CiteprocItem[]): void {
        for (const item of items) {
            if (!item.id) {
                throw new Error(
                    '@autonomics/citation-engine: CslItem must have a non-empty `id`.',
                );
            }
            this.items.set(item.id, item);
        }
        // Prime citeproc-js's internal registry. The engine looks items up via
        // the sys.retrieveItem callback we bound at construction; `updateItems`
        // is the documented way to tell it which ids exist (it'll call
        // retrieveItem internally). Idempotent on repeat ids.
        this.proc.updateItems([...this.items.keys()]);
    }

    /**
     * Render a single in-text citation cluster for the given item ids.
     *
     * @param itemIds  1+ registered item ids to include in this cluster.
     * @param locators Optional locators per item (page numbers, etc).
     * @returns        Rendered HTML (or plain text per style).
     *
     * Uses `makeCitationCluster` (simpler) rather than `processCitationCluster`
     * (which tracks first/subsequent form across multiple calls — needed only
     * when rendering a whole document's citations in order).
     */
    citeCluster(
        itemIds: string[],
        locators?: CitationClusterLocator[],
    ): CitationResult {
        if (itemIds.length === 0) {
            return { html: '' };
        }
        const locatorById = new Map(
            (locators ?? []).map(l => [l.id, l] as const),
        );
        const cluster = itemIds.map(id => {
            const loc = locatorById.get(id);
            return {
                id,
                ...(loc?.locator ? { locator: loc.locator } : {}),
                ...(loc?.label ? { label: loc.label } : {}),
            };
        });
        const html = this.proc.makeCitationCluster(cluster);
        return { html };
    }

    /**
     * Render the full bibliography of all registered items.
     *
     * @param mode 'html' (default) | 'text' | 'rtf' — matches citeproc-js modes.
     */
    bibliography(mode: 'html' | 'text' | 'rtf' = 'html'): BibliographyResult {
        // citeproc-js returns `[params, entries]` — a 2-tuple, not an object.
        // params holds bibstart/bibend/entry_ids; entries is the rendered HTML strings.
        const [params, entries] = this.proc.makeBibliography(mode) as [
            BibliographyParams,
            string[],
        ];
        return {
            bibstart: params?.bibstart ?? '',
            bibend: params?.bibend ?? '',
            entries: entries ?? [],
        };
    }
}
