/**
 * React hooks for using CitationEngine in components.
 *
 * Two hooks:
 * - `useCitationEngine(styleId)` — owns the engine instance for one style.
 *   Uses a process-wide LRU pool so multiple components sharing a style reuse
 *   the same engine (citeproc-js startup is ~50-200ms; we don't want to pay
 *   it per component).
 * - `useCitation(itemId, styleId?)` — convenience hook that fetches the CSL
 *   JSON via a callback (caller-provided) and renders the citation string.
 */

import { useEffect, useState } from 'react';

import { CitationEngine } from './engine';
import type { CiteprocItem, CitationClusterLocator } from './engine';
import { DEFAULT_STYLE_ID, getBundledStyle } from './styles';
import { getLocaleXml } from './locales';

/**
 * Process-wide LRU pool of CitationEngine instances, keyed by styleId.
 *
 * citeproc-js engine construction is ~50-200ms; we cap the pool at 3 to avoid
 * unbounded memory growth if the user cycles through many styles. LRU eviction
 * order is preserved by the Map's insertion-order iteration.
 */
const ENGINE_POOL = new Map<string, CitationEngine>();
const POOL_MAX_SIZE = 3;

function evictIfNeeded(): void {
    while (ENGINE_POOL.size >= POOL_MAX_SIZE) {
        // Map iteration is insertion-order; first key is least-recently-used.
        const oldest = ENGINE_POOL.keys().next().value;
        if (oldest === undefined) break;
        ENGINE_POOL.delete(oldest);
    }
}

/**
 * Get-or-create an engine for `styleId`. Engines are cached process-wide.
 *
 * Re-accessing an engine moves it to the end of the Map (most-recently-used)
 * by deleting + re-inserting — JavaScript Map preserves insertion order, so
 * the first key is always the LRU candidate.
 */
export function getEngineForStyle(styleId: string = DEFAULT_STYLE_ID): CitationEngine {
    const existing = ENGINE_POOL.get(styleId);
    if (existing) {
        // Move to end (most recently used).
        ENGINE_POOL.delete(styleId);
        ENGINE_POOL.set(styleId, existing);
        return existing;
    }
    const style = getBundledStyle(styleId);
    const localeXml = getLocaleXml(style.locale);
    evictIfNeeded();
    const engine = new CitationEngine(style.cslXml, localeXml);
    ENGINE_POOL.set(styleId, engine);
    return engine;
}

/** Test-only: clear the pool. Exported for unit tests. */
export function _clearEnginePoolForTests(): void {
    ENGINE_POOL.clear();
}

/** Hook result for engine loading. */
export interface UseCitationEngineResult {
    engine: CitationEngine | null;
    loading: boolean;
    error: Error | null;
}

/**
 * Get a ready-to-use CitationEngine for the given style.
 *
 * The engine is loaded asynchronously on mount to keep initial page paint
 * cheap (citeproc-js + style XML + locale XML together are ~500KB).
 *
 * @param styleId  One of BUNDLED_STYLES ids. Defaults to GB/T 7714 (zh-CN).
 */
export function useCitationEngine(styleId: string = DEFAULT_STYLE_ID): UseCitationEngineResult {
    const [engine, setEngine] = useState<CitationEngine | null>(null);
    const [error, setError] = useState<Error | null>(null);

    useEffect(() => {
        let cancelled = false;
        // Defer engine creation to next tick so we don't block the current
        // render cycle with citeproc-js's ~50-200ms constructor.
        // The dynamic import would be ideal here, but `citeproc` is already
        // a normal dep — the pool caches instances anyway, so the cost is paid
        // exactly once per style per session.
        setEngine(null);
        setError(null);
        Promise.resolve().then(() => {
            if (cancelled) return;
            try {
                setEngine(getEngineForStyle(styleId));
            } catch (e) {
                setError(e instanceof Error ? e : new Error(String(e)));
            }
        });
        return () => {
            cancelled = true;
        };
    }, [styleId]);

    return { engine, loading: engine === null && error === null, error };
}

/** Hook result for single-citation rendering. */
export interface UseCitationResult {
    /** Rendered citation HTML/string; null while loading. */
    text: string | null;
    loading: boolean;
    error: Error | null;
}

/**
 * Fetch a CSL-JSON item and render it in the chosen style.
 *
 * @param itemId       Item id (e.g. UUID from jayread DB).
 * @param fetchItem    Callback that returns the CSL-JSON item. Caller injects
 *                     this so the engine package doesn't depend on the web
 *                     app's HTTP client. The callback should cache the result
 *                     upstream if appropriate.
 * @param options      styleId + optional locator (page number etc).
 */
export function useCitation(
    itemId: string | null | undefined,
    fetchItem: (itemId: string) => Promise<CiteprocItem>,
    options: {
        styleId?: string;
        locator?: CitationClusterLocator;
    } = {},
): UseCitationResult {
    const styleId = options.styleId ?? DEFAULT_STYLE_ID;
    const { engine, error: engineError } = useCitationEngine(styleId);
    const [text, setText] = useState<string | null>(null);
    const [fetchError, setFetchError] = useState<Error | null>(null);

    useEffect(() => {
        if (!itemId || !engine) {
            setText(null);
            return;
        }
        let cancelled = false;
        setFetchError(null);
        fetchItem(itemId)
            .then(item => {
                if (cancelled) return;
                engine.addItems([item]);
                const { html } = engine.citeCluster(
                    [item.id],
                    options.locator ? [options.locator] : undefined,
                );
                setText(html);
            })
            .catch(e => {
                if (!cancelled) {
                    setFetchError(e instanceof Error ? e : new Error(String(e)));
                }
            });
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [itemId, engine, styleId, options.locator?.locator, options.locator?.label]);

    return {
        text,
        loading: itemId != null && engine != null && text === null && fetchError === null,
        error: engineError ?? fetchError,
    };
}
