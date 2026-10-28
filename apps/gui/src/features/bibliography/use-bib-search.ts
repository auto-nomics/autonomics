import { useCallback, useRef } from "react";
import { gatewayRequest } from "@/gateway";
import type { LibraryMetadata, SearchPage, SearchRequest } from "./bib-search-model";

/** Shared query transport and independently cached library metadata. The view
 * owns selection/scroll state; request IDs protect it even when a transport
 * cannot interrupt an operation that is already executing. */
export function useBibSearch() {
  const metadata = useRef<{ value?: LibraryMetadata; expires: number; pending?: Promise<LibraryMetadata> }>({ expires: 0 });
  const invalidateMetadata = useCallback(() => { metadata.current = { value: metadata.current.value, expires: 0 }; }, []);
  const libraryMetadata = useCallback(async (): Promise<LibraryMetadata> => {
    const cache = metadata.current;
    if (cache.value && cache.expires > Date.now()) return cache.value;
    if (cache.pending) return cache.pending;
    const pending = Promise.all([
      gatewayRequest("GET", "/bib/collections?include_deleted=true&view=summary"),
      gatewayRequest("GET", "/bib/health"),
    ]).then(([collections, health]) => {
      const value = { collections: collections.collections ?? [], total: health.article_count ?? 0 };
      cache.value = value;
      cache.expires = Date.now() + 60_000;
      return value;
    }).finally(() => { cache.pending = undefined; });
    cache.pending = pending;
    return pending;
  }, []);
  const searchPage = useCallback(async (request: SearchRequest, signal?: AbortSignal): Promise<SearchPage> => {
    return gatewayRequest("POST", "/bib/search/v2", { ...request, page_size: 50, include_articles: true, include_facets: true }, { signal });
  }, []);
  return { searchPage, libraryMetadata, invalidateMetadata };
}
