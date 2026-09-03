/**
 * @autonomics/citation-engine — citeproc-js wrapper for client-side CSL rendering.
 *
 * Public API:
 * - `CitationEngine` class (engine.ts): wraps citeproc-js for one CSL style
 * - `useCitationEngine` / `useCitation` hooks (react.ts): lazy-load + LRU pool
 * - Bundled CSL styles (styles.ts) + locales (locales.ts): 5 styles + 2 locales
 *
 * See ../../docs or the workspace plan (abundant-squishing-quokka.md §九) for
 * the design rationale: bundle all resources (~500KB) for offline use, lazy-load
 * the engine only when a citation is actually requested.
 */

// 引入 citeproc ambient 类型声明。三斜杠引用让消费方（@autonomics/web）
// 在编译本包源码时也能看到 `declare module 'citeproc'`，避免 TS7016。
/// <reference path="../types/citeproc.d.ts" />

export { CitationEngine } from './engine';
export type {
    CitationResult,
    BibliographyResult,
    CiteprocItem,
    CitationClusterLocator,
} from './engine';
export * from './styles';
export * from './locales';
