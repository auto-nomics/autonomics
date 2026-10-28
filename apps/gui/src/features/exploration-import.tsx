import { useEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";
import { useDiscovery } from "./use-discovery";
import type { DiscoveryRequest, DiscoveryImportRequest, DiscoveryImportResult, DiscoveryPreview } from "./discovery-model";
import type { BibArticle } from "@/types";
export type ExplorationImportTarget = { collection_id: string; library_id?: never } | { library_id: number; collection_id?: never };

/** Re-fetch a selected graph identity through the existing exact discovery
 * session, then use its fingerprinted preview/commit APIs. No result metadata
 * from the graph UI is accepted as an import payload. */
export function ExplorationImportReview({ paperId, target, targetLabel, onClose, onImported }: { paperId: string; target: ExplorationImportTarget; targetLabel: string; onClose: () => void; onImported?: () => void }) {
  return <IdentityImportReview key={paperId} lookup={{ sources: ["semantic_scholar"], native: { source: "semantic_scholar", parameters: { endpoint: "lookup", paper_id: paperId } }, page_size: 1 }} lookupLabel="Semantic Scholar · exact paper lookup" target={target} targetLabel={targetLabel} onClose={onClose} onImported={onImported} />;
}

export function IdentityImportReview({ lookup, lookupLabel, accepts, target, targetLabel, onClose, onImported }: { lookup: DiscoveryRequest; lookupLabel: string; accepts?: (article: BibArticle) => boolean; target: ExplorationImportTarget; targetLabel: string; onClose: () => void; onImported?: () => void }) {
  const [identity] = useState(() => ({ request: structuredClone(lookup), label: lookupLabel, accepts }));
  const discovery = useDiscovery();
  const [destination] = useState(() => structuredClone(target));
  const [destinationLabel] = useState(targetLabel);
  const [previewState, setPreviewState] = useState<{ key: string; value: DiscoveryPreview } | null>(null);
  const [outcome, setOutcome] = useState<DiscoveryImportResult | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const operation = useRef<AbortController | null>(null);
  useEffect(() => {
    void discovery.start(identity.request);
    return () => operation.current?.abort();
  }, [identity, discovery.start]);
  const candidate = discovery.candidates.find(candidate => !identity.accepts || identity.accepts(candidate.article));
  const sessionId = discovery.session?.id;
  const request: DiscoveryImportRequest = useMemo(() => ({ ...destination, fetch_fulltext: false, selections: candidate ? [{ candidate_id: candidate.id, expected_fingerprint: candidate.fingerprint, record_index: 0 }] : [] }), [candidate, destination]);
  const key = JSON.stringify({ sessionId, request });
  const preview = previewState?.key === key ? previewState.value : null;
  async function act(commit: boolean) {
    if (!sessionId || !candidate || (commit && (!preview?.editable || outcome))) return;
    operation.current?.abort(); const abort = new AbortController(); operation.current = abort; setBusy(true); setError("");
    try {
      const response = await gatewayRequest("POST", `/bib/search/sessions/${encodeURIComponent(sessionId)}/import${commit ? "" : "/preview"}`, request, { signal: abort.signal });
      if (abort.signal.aborted) return;
      if (commit) { setOutcome(response); setPreviewState(null); onImported?.(); }
      else setPreviewState({ key, value: response.preview });
    } catch (failure) { if (!abort.signal.aborted) { setPreviewState(null); setError(`${String(failure)}. Preview metadata again before confirming.`); discovery.refresh(); } }
    finally { if (operation.current === abort) setBusy(false); }
  }
  return <SearchDialog title="Review paper import" onClose={onClose}>
    <p className="text-xs text-muted-foreground">Fresh metadata is fetched for the selected paper. Existing library metadata is preserved.</p>
    <p className="mt-2 text-xs">Import target: {destinationLabel}</p>
    {(error || discovery.error) && <p role="alert" className="mt-2 text-xs text-destructive">{error || discovery.error}</p>}
    {discovery.session?.sources.map(source => source.failure && <p key={source.source} className="mt-2 text-xs text-destructive">{source.failure.message}</p>)}
    {discovery.creating || (!candidate && discovery.session?.phase === "running") ? <p role="status" className="mt-2">Fetching current paper metadata…</p> : candidate ? <div className="mt-3 rounded border p-2"><h3 className="font-medium">{candidate.article.title}</h3>{candidate.article.year && <p className="text-xs">{candidate.article.year}</p>}<p className="text-xs text-muted-foreground">{identity.label}</p></div> : <p className="mt-2 text-xs">No verified paper was obtained. Close this review and retry the source.</p>}
    {preview && <div className="mt-3 space-y-2 text-xs">
      {!preview.editable && <p>Import target is read only.</p>}
      {preview.items.map(item => <div key={item.candidate_id} className="rounded border p-2"><p>{item.existing.length ? `Already in library: ${item.existing.map(article => article.title).join("; ")}` : "New library record"}</p>{item.related_existing.length > 0 && <p>Related version: {item.related_existing.map(article => article.title).join("; ")}</p>}{item.differences.length > 0 && <details><summary>Metadata differences</summary><pre className="whitespace-pre-wrap break-words">{JSON.stringify(item.differences, null, 2)}</pre></details>}{item.error && <p className="text-destructive">{item.error}</p>}</div>)}
    </div>}
    {outcome && <div role="status" className="mt-3 text-xs"><p>{outcome.imported} imported · {outcome.failed} failed</p>{outcome.items.map(item => <p key={item.candidate_id}>{item.error ?? item.status}</p>)}</div>}
    <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>{outcome ? "Done" : "Cancel"}</Button>{!outcome && <><Button size="sm" variant="outline" disabled={busy || !candidate} onClick={() => void act(false)}>Preview paper import</Button><Button size="sm" disabled={busy || !preview?.editable || preview.items.every(item => Boolean(item.error))} onClick={() => void act(true)}>Confirm paper import</Button></>}</div>
  </SearchDialog>;
}
