import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { DiscoveryRecord, DiscoveryRequest } from "./discovery-model";

type Page = {
  total: number; next_offset: number | null; related_article_ids: string[];
  entries: Array<{ imported_at: string; provenance: { selected: DiscoveryRecord; records: DiscoveryRecord[]; request: DiscoveryRequest } }>;
};
/** Read only when expanded; imported provenance outlives search sessions. */
export function DiscoveryProvenance({ articleId }: { articleId: string }) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<Page | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const currentId = useRef(articleId); currentId.current = articleId;
  const load = async (offset = 0) => {
    abortRef.current?.abort(); const abort = new AbortController(); abortRef.current = abort;
    setLoading(true); setError("");
    try {
      const next: Page = await gatewayRequest("GET", `/bib/articles/${encodeURIComponent(articleId)}/discovery-provenance?offset=${offset}&limit=5`, undefined, { signal: abort.signal });
      if (abort.signal.aborted || currentId.current !== articleId) return;
      setPage(previous => offset && previous ? { ...next, entries: [...previous.entries, ...next.entries] } : next);
    } catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
    finally { if (abortRef.current === abort) setLoading(false); }
  };
  useEffect(() => { abortRef.current?.abort(); setPage(null); setOpen(false); setError(""); setLoading(false); return () => abortRef.current?.abort(); }, [articleId]);
  return <details open={open} onToggle={event => { const expanded = event.currentTarget.open; setOpen(expanded); if (expanded && !page && !loading) void load(); }} className="rounded border p-2 text-xs">
    <summary className="cursor-pointer font-medium">Discovery provenance{page ? ` (${page.total})` : ""}</summary>
    {loading && <p role="status" className="mt-2">Loading retrieval history…</p>}
    {error && <p role="alert" className="mt-2 text-destructive">{error}<Button size="xs" variant="ghost" onClick={() => void load()}>Retry</Button></p>}
    {page?.total === 0 && <p className="mt-2 text-muted-foreground">No discovery import history.</p>}
    {page?.entries.map((entry, index) => <div key={index} className="mt-2 space-y-1 border-t pt-2">
      <p>{entry.provenance.selected.source} · imported {new Date(entry.imported_at).toLocaleString()}</p>
      <p className="font-medium">{entry.provenance.selected.article.title}</p>
      <p className="text-muted-foreground">{entry.provenance.records.map(record => `${record.source} #${record.source_rank}`).join(" · ")}</p>
      <details><summary className="cursor-pointer">Original query and source execution</summary><pre className="mt-1 whitespace-pre-wrap break-all">{JSON.stringify({ request: entry.provenance.request, sources: entry.provenance.records.map(record => ({ source: record.source, retrieved_at: record.retrieved_at, parameters: record.executed_parameters, translation: record.query_translation })) }, null, 2)}</pre></details>
    </div>)}
    {Boolean(page?.related_article_ids.length) && <p className="mt-2 text-muted-foreground">{page!.related_article_ids.length} linked publication version(s) retained in this library.</p>}
    {page?.next_offset !== null && page?.next_offset !== undefined && <Button size="xs" variant="outline" disabled={loading} onClick={() => void load(page.next_offset!)}>Load more history</Button>}
  </details>;
}
