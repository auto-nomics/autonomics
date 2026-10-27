import { useEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { BibCollection } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AdvancedSearchBuilder } from "./advanced-search-builder";
import { VirtualArticleList } from "./virtual-article-list";
import { useDiscovery } from "./use-discovery";
import { useDiscoveryPresence } from "./use-discovery-presence";
import type { SearchExpr } from "./bib-search-model";
import type { DiscoveryAcquisition, DiscoveryCandidate, DiscoveryImportRequest, DiscoveryImportResult, DiscoveryOrder, DiscoveryPreview, DiscoveryRequest, DiscoverySelection, SourceCapabilities } from "./discovery-model";

const activePhase = (phase: string) => ["running", "queued", "waiting_retry"].includes(phase);
const pretty = (value: unknown) => JSON.stringify(value, null, 2);
const authenticationLabel = (source: SourceCapabilities) => {
  if (source.authentication === "none") return "public API · no key needed";
  if (source.credentials_configured) return "API key configured";
  if (source.authentication === "optional") return "API key optional";
  if (source.authentication === "required_for_continuation") return "API key required for more pages";
  if (source.authentication === "required") return "API key required";
  return "authentication policy not reported";
};
const identifierLink = (kind: string, value: string) => {
  const id = encodeURIComponent(value.trim());
  switch (kind.toLowerCase()) {
    case "doi": return `https://doi.org/${value.trim().replace(/^(?:https?:\/\/(?:dx\.)?doi\.org\/|doi:\s*)/i, "").split("/").map(encodeURIComponent).join("/")}`;
    case "pmid": return `https://pubmed.ncbi.nlm.nih.gov/${id}/`;
    case "pmc": return `https://pmc.ncbi.nlm.nih.gov/articles/${id}/`;
    case "arxiv": return `https://arxiv.org/abs/${id}`;
    default: return null;
  }
};

export function DiscoveryView() {
  const discovery = useDiscovery();
  const [capabilities, setCapabilities] = useState<SourceCapabilities[]>([]);
  const [sourceRuntime, setSourceRuntime] = useState<Array<{ source: string; occupied_slots: number; concurrency_limit: number; cooldown_remaining_ms: number }>>([]);
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [sources, setSources] = useState(["pubmed", "openalex"]);
  const [query, setQuery] = useState("");
  const [expression, setExpression] = useState<SearchExpr | null>(null);
  const [native, setNative] = useState(false);
  const [nativeParameters, setNativeParameters] = useState('{"term":""}');
  const nativeDrafts = useRef<Record<string, string>>({});
  const [yearFrom, setYearFrom] = useState("");
  const [yearTo, setYearTo] = useState("");
  const [publicationType, setPublicationType] = useState("");
  const [openAccess, setOpenAccess] = useState(false);
  const [allowApproximate, setAllowApproximate] = useState(false);
  const [sourceSort, setSourceSort] = useState<NonNullable<DiscoveryRequest["sort"]>>("relevance");
  const [advanced, setAdvanced] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [variants, setVariants] = useState<Record<string, number>>({});
  const [selections, setSelections] = useState<DiscoverySelection[]>([]);
  const [collection, setCollection] = useState("");
  const [fetchFulltext, setFetchFulltext] = useState(false);
  const [previewState, setPreviewState] = useState<{ key: string; sessionId: string; value: DiscoveryPreview } | null>(null);
  const [outcome, setOutcome] = useState<DiscoveryImportResult | null>(null);
  const [acquisition, setAcquisition] = useState<DiscoveryAcquisition | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const viewport = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLElement>(null);
  const operation = useRef<AbortController | null>(null);
  const request: DiscoveryImportRequest = useMemo(() => ({ selections, ...(collection ? { collection_id: collection } : { library_id: 1 }), fetch_fulltext: fetchFulltext }), [selections, collection, fetchFulltext]);
  const requestKey = JSON.stringify(request);
  const sessionId = discovery.session?.id;
  const presence = useDiscoveryPresence(sessionId, discovery.candidates, collection, variants, discovery.filter.source);
  const preview = previewState?.key === requestKey && previewState.sessionId === sessionId ? previewState.value : null;

  useEffect(() => {
    const abort = new AbortController();
    void Promise.all([
      gatewayRequest("GET", "/bib/search/sources", undefined, { signal: abort.signal }),
      gatewayRequest("GET", "/bib/collections?view=summary", undefined, { signal: abort.signal }),
    ]).then(([sourceResponse, collectionResponse]) => {
      setCapabilities(sourceResponse.sources); setSourceRuntime(sourceResponse.runtime ?? []); setCollections(collectionResponse.collections ?? []);
    }).catch(failure => { if (!abort.signal.aborted) setError(String(failure)); });
    return () => abort.abort();
  }, []);
  useEffect(() => {
    if (!sessionId || discovery.session?.phase === "running") return;
    const abort = new AbortController();
    void gatewayRequest("GET", "/bib/search/sources", undefined, { signal: abort.signal }).then(response => {
      setCapabilities(response.sources); setSourceRuntime(response.runtime ?? []);
    }).catch(() => {});
    return () => abort.abort();
  }, [sessionId, discovery.session?.phase]);
  useEffect(() => {
    operation.current?.abort(); setBusy(false); setPreviewState(null);
  }, [requestKey, sessionId]);
  useEffect(() => () => operation.current?.abort(), []);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!root.current?.getClientRects().length || document.querySelector("dialog[open],.zotero-pdf-reader,.zotero-txt-reader")) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault(); root.current.querySelector<HTMLInputElement | HTMLTextAreaElement>(native ? "textarea" : 'input[aria-label="Discover literature"]')?.focus();
      }
      if (event.key === "Escape") { setSourcesOpen(false); setDetailsOpen(false); }
    };
    window.addEventListener("keydown", onKey); return () => window.removeEventListener("keydown", onKey);
  }, [native]);
  useEffect(() => {
    const id = acquisition?.id;
    if (!id || acquisition.completed_at) return;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response: { acquisition: DiscoveryAcquisition } = await gatewayRequest("GET", `/bib/search/acquisitions/${encodeURIComponent(id)}`, undefined, { signal: abort.signal });
        setAcquisition(response.acquisition);
        if (!response.acquisition.completed_at) timer = setTimeout(() => void poll(), 1500);
      } catch (failure) { if (!abort.signal.aborted) setError(`Full-text progress: ${String(failure)}`); }
    };
    void poll();
    return () => { abort.abort(); clearTimeout(timer); };
  }, [acquisition?.id, acquisition?.completed_at]);

  const defaultVariant = (candidate: DiscoveryCandidate) => variants[candidate.id] ?? Math.max(0, candidate.records.findIndex(record => record.source === discovery.filter.source));
  const toggle = (candidateId: string) => {
    const candidate = discovery.candidates.find(item => item.id === candidateId);
    if (!candidate) return;
    setSelections(current => current.some(item => item.candidate_id === candidateId) ? current.filter(item => item.candidate_id !== candidateId) : current.length >= 100 ? current : [...current, { candidate_id: candidateId, expected_fingerprint: candidate.fingerprint, record_index: defaultVariant(candidate) }]);
  };
  const selected = discovery.candidates.find(item => item.id === selectedId);
  const article = selected?.records[defaultVariant(selected)]?.article ?? selected?.article;
  const chooseNativeSource = (source: string) => {
    const key = source === "pubmed" ? "term" : source === "arxiv" ? "search_query" : source === "crossref" ? "query.bibliographic" : source === "semantic_scholar" ? "query" : "search";
    const draft = nativeDrafts.current[source] ?? pretty({ [key]: query });
    nativeDrafts.current[source] = draft;
    setSources([source]); setNativeParameters(draft);
  };
  const selectSource = (source: string, checked: boolean) => {
    if (native && checked) chooseNativeSource(source);
    else setSources(current => checked ? [...current, source] : current.filter(name => name !== source));
  };
  const submit = () => {
    setError("");
    try {
      let search: DiscoveryRequest;
      if (native) {
        const parameters: unknown = JSON.parse(nativeParameters);
        if (!parameters || typeof parameters !== "object" || Array.isArray(parameters) || Object.values(parameters).some(value => typeof value !== "string")) throw new Error("Native parameters must be a JSON object of string values.");
        if (sources.length !== 1) throw new Error("Choose exactly one source for native search.");
        search = { sources, native: { source: sources[0], parameters: parameters as Record<string, string> } };
      } else {
        for (const value of [yearFrom, yearTo]) if (value && (!/^\d{1,4}$/.test(value) || Number(value) > 9999)) throw new Error("Enter a valid publication year.");
        if (yearFrom && yearTo && Number(yearFrom) > Number(yearTo)) throw new Error("The first year must not be after the last year.");
        search = { sources, ...(expression ? { expression } : { query }), allow_approximate: allowApproximate, filters: { ...(yearFrom ? { year_from: Number(yearFrom) } : {}), ...(yearTo ? { year_to: Number(yearTo) } : {}), publication_types: publicationType ? [publicationType] : [], ...(openAccess ? { open_access: true } : {}) } };
      }
      setSelections([]); setVariants({}); setSelectedId(null); setOutcome(null);
      void discovery.start({ ...search, sort: sourceSort });
    } catch (failure) { setError(String(failure)); }
  };
  const importAction = async (commit: boolean) => {
    if (!sessionId) return;
    if (commit && (!preview || !preview.editable)) return;
    operation.current?.abort(); const abort = new AbortController(); operation.current = abort;
    setBusy(true); setError("");
    try {
      const response = await gatewayRequest("POST", `/bib/search/sessions/${encodeURIComponent(sessionId)}/import${commit ? "" : "/preview"}`, request, { signal: abort.signal });
      if (abort.signal.aborted) return;
      if (commit) { setOutcome(response); setAcquisition(response.acquisition); setPreviewState(null); presence.refresh(); }
      else setPreviewState({ key: requestKey, sessionId, value: response.preview });
    } catch (failure) { if (!abort.signal.aborted) { setPreviewState(null); setError(String(failure)); } }
    finally { if (operation.current === abort) setBusy(false); }
  };
  const articles = useMemo(() => discovery.candidates.map(candidate => ({ ...candidate.article, id: candidate.id })), [discovery.candidates]);
  const byId = useMemo(() => new Map(discovery.candidates.map(candidate => [candidate.id, candidate])), [discovery.candidates]);

  return <section ref={root} aria-label="Literature discovery" className="flex min-h-0 flex-1 flex-col gap-2">
    {advanced && <AdvancedSearchBuilder query={query} expression={expression} onClose={() => setAdvanced(false)} onApply={(expr, text) => { setExpression(expr); setQuery(text); setAdvanced(false); }} />}
    <form className="space-y-2 rounded border p-3" onSubmit={event => { event.preventDefault(); submit(); }}>
      <div className="flex flex-wrap gap-2">
        <Input aria-label="Discover literature" className="min-w-40 flex-1" placeholder="Title, author, DOI, or a research topic" value={query} disabled={native} onChange={event => { setQuery(event.target.value); setExpression(null); }} />
        <Button type="button" variant="secondary" disabled={native} onClick={() => setAdvanced(true)}>Advanced</Button>
        <Button type="submit" disabled={!sources.length || discovery.creating}>Search sources</Button>
        {(discovery.creating || discovery.session?.phase === "running") && <Button type="button" variant="outline" onClick={() => discovery.creating ? discovery.cancelCreation() : void discovery.sourceAction("cancel")}>Stop search</Button>}
      </div>
      <div className="flex flex-wrap items-center gap-3 text-xs">
        {capabilities.map(source => <label key={source.source} title={source.notes.join(" ")} className="flex items-center gap-1"><input type={native ? "radio" : "checkbox"} name={native ? "discovery-native-source" : undefined} checked={sources.includes(source.source)} disabled={!source.searchable || (native && !source.native_query)} onChange={event => selectSource(source.source,event.target.checked)} />{source.source}{!source.searchable && " (unavailable for discovery)"}</label>)}
        <label className="flex items-center gap-1"><input type="checkbox" checked={native} disabled={!native && !capabilities.some(source => source.searchable && source.native_query)} onChange={event => { setNative(event.target.checked); if (event.target.checked) chooseNativeSource(sources.find(name => capabilities.some(cap => cap.source === name && cap.native_query)) ?? "pubmed"); }} />Native query</label>
        <label className="flex items-center gap-1">Source order<select aria-label="Discovery source order" className="rounded border bg-background p-1" value={sourceSort} onChange={event => setSourceSort(event.target.value as NonNullable<DiscoveryRequest["sort"]>)}><option value="relevance">Relevance / source default</option><option value="newest">Newest</option><option value="oldest">Oldest</option><option value="citation_count">Citation count</option></select></label>
      </div>
      {native ? <label className="block text-xs">Source parameters<span className="my-1 block text-muted-foreground">This mode runs only the native parameters below. Standard search conditions are retained in their own draft.</span><textarea aria-label="Native source parameters" className="mt-1 block w-full rounded border bg-background p-2 font-mono" rows={3} value={nativeParameters} onChange={event => { setNativeParameters(event.target.value); if(sources.length===1) nativeDrafts.current[sources[0]]=event.target.value; }} /></label> : <div className="flex flex-wrap items-center gap-2 text-xs">
        <Input aria-label="Discovery year from" className="w-28" placeholder="From year" inputMode="numeric" value={yearFrom} onChange={event => setYearFrom(event.target.value)} />
        <Input aria-label="Discovery year to" className="w-28" placeholder="To year" inputMode="numeric" value={yearTo} onChange={event => setYearTo(event.target.value)} />
        <select aria-label="Discovery publication type" className="rounded border bg-background p-2" value={publicationType} onChange={event => setPublicationType(event.target.value)}><option value="">All types</option><option value="article">Journal article</option><option value="review">Review</option><option value="preprint">Preprint</option><option value="clinical_trial">Clinical trial</option></select>
        <label className="flex items-center gap-1"><input type="checkbox" checked={openAccess} onChange={event => setOpenAccess(event.target.checked)} />Open access only</label>
        <label className="flex items-center gap-1" title="Permit disclosed loose text matching where the source cannot enforce Boolean or exact phrase matching. Identifiers, filters and exclusions remain protected."><input type="checkbox" checked={allowApproximate} onChange={event => setAllowApproximate(event.target.checked)} />Allow approximate source matching</label>
        <span className="text-muted-foreground">Unsupported conditions are reported per source.</span>
      </div>}
    </form>
    {(error || discovery.error) && <div role="alert" className="flex items-center gap-2 rounded border p-2 text-xs text-destructive"><span>{error || discovery.error}</span>{discovery.error && <Button size="xs" variant="outline" onClick={discovery.resume}>Resume progress</Button>}</div>}
    <div className="flex gap-2 lg:hidden"><Button size="xs" variant="outline" onClick={() => setSourcesOpen(value => !value)}>Source status</Button><Button size="xs" variant="outline" disabled={!selected} onClick={() => setDetailsOpen(value => !value)}>Candidate details</Button></div>
    <div className="relative grid min-h-0 flex-1 grid-cols-1 grid-rows-[minmax(0,1fr)] gap-2 lg:grid-cols-[230px_minmax(0,1fr)_minmax(250px,32%)]">
      <aside aria-label="Discovery sources" className={`${sourcesOpen ? "absolute inset-0 z-10 block lg:static" : "hidden lg:block"} overflow-auto rounded border bg-background p-2 text-xs`}>
        <Button size="xs" variant="outline" className="mb-2 lg:hidden" onClick={() => setSourcesOpen(false)}>Close source status</Button>
        {!discovery.session && <p>Choose sources and submit a search. External records enter your library when imported.</p>}
        {discovery.session?.sources.map(source => <section key={source.source} className="mb-3 space-y-1 border-b pb-3">
          <div className="flex justify-between font-medium"><span>{source.source}</span><span>{source.phase}</span></div>
          <p>{source.delivered_records} obtained{source.total !== null && ` · ${source.total.toLocaleString()} reported by source`}</p>
          {source.failure && <p className="text-destructive">{source.failure.kind}: {source.failure.message}</p>}
          {source.notices.map((notice, index) => <p key={index} className="text-muted-foreground">{notice.message}</p>)}
          <div className="flex flex-wrap gap-1">
            {activePhase(source.phase) && <Button size="xs" variant="outline" onClick={() => void discovery.sourceAction("cancel", source.source)}>Stop</Button>}
            {source.has_more && !activePhase(source.phase) && source.phase === "ready" && <Button size="xs" variant="outline" onClick={() => void discovery.sourceAction("next", source.source)}>Fetch next page</Button>}
            {["failed", "cancelled"].includes(source.phase) && source.plan && <Button size="xs" variant="outline" onClick={() => void discovery.sourceAction("retry", source.source)}>Retry source</Button>}
          </div>
          <details><summary className="cursor-pointer">Query details</summary><pre className="mt-1 whitespace-pre-wrap break-all">{pretty({ planned: source.plan?.parameters, executed: source.executed_parameters, translation: source.query_translation })}</pre></details>
        </section>)}
        {capabilities.filter(cap => sources.includes(cap.source)).map(cap => <p className="mb-2 text-muted-foreground" key={cap.source}>{cap.source}: {authenticationLabel(cap)}{cap.quota && ` · last reported budget ${cap.quota.remaining ?? "unknown"}/${cap.quota.limit ?? "unknown"}`}</p>)}
        {sourceRuntime.filter(source => sources.includes(source.source)).map(source => <p key={source.source} className="mb-1 text-muted-foreground">{source.source}: {source.occupied_slots}/{source.concurrency_limit} requests active{source.cooldown_remaining_ms > 0 && ` · cooldown ${Math.ceil(source.cooldown_remaining_ms / 1000)}s (last observed)`}</p>)}
      </aside>
      <div className="flex min-h-0 flex-col rounded border">
        <div className="flex flex-wrap items-center gap-2 border-b p-2 text-xs">
          <span role="status">{discovery.creating ? "Starting search…" : `${discovery.session?.merged_candidates ?? 0} obtained candidates`}</span>
          <select aria-label="Discovery result source" className="rounded border bg-background p-1" value={discovery.filter.source} onChange={event => discovery.changeFilter(event.target.value, discovery.filter.order)}><option value="">All sources</option>{discovery.session?.sources.map(source => <option key={source.source} value={source.source}>{source.source}</option>)}</select>
          <select aria-label="Discovery result order" className="rounded border bg-background p-1" value={discovery.filter.order} onChange={event => discovery.changeFilter(discovery.filter.source, event.target.value as DiscoveryOrder)}><option value="first_seen">First obtained</option><option value="relevance">Combined source rank</option><option value="newest">Newest obtained</option><option value="source_rank">Source rank</option></select>
          <Button size="xs" variant="outline" disabled={!sessionId || discovery.loadingResults} onClick={() => { discovery.refresh(); presence.refresh(); }}>{discovery.updatesPending ? "Show updated results" : "Refresh results"}</Button>
        </div>
        <div ref={viewport} tabIndex={0} aria-label="Discovery result list" className="min-h-0 flex-1 overflow-auto">
          <VirtualArticleList articles={articles} total={discovery.obtainedCandidates} viewport={viewport} contextKey={`${sessionId ?? "empty"}:${discovery.filter.source}:${discovery.filter.order}`} estimatedHeight={145} selectedId={selectedId} onNavigate={setSelectedId} onToggle={toggle}>{row => {
            const candidate = byId.get(row.id)!;
            const selected = selections.some(item => item.candidate_id === row.id);
            return <div className={`border-b p-3 ${selectedId === row.id ? "bg-muted" : ""}`}>
              <div className="flex items-start gap-2"><input type="checkbox" aria-label={`Select ${row.title}`} checked={selected} disabled={!selected && selections.length >= 100} onChange={() => toggle(row.id)} /><button type="button" data-bib-row-primary className="min-w-0 flex-1 text-left font-medium" onClick={() => { setSelectedId(row.id); setDetailsOpen(true); }}>{row.title}</button></div>
              <p className="mt-1 text-xs text-muted-foreground">{row.year ?? "Year unknown"} · {row.journal ?? "Publication unknown"}</p>
              <p className="mt-1 line-clamp-2 text-xs">{row.abstract_text}</p><p className={`mt-1 text-xs ${presence.entries[row.id]?.error ? "text-muted-foreground" : "text-primary"}`}>{presence.entries[row.id]?.label}</p>
              <p className="mt-1 text-xs text-muted-foreground">{candidate.records.map(record => `${record.source} #${record.source_rank}`).join(" · ")}{candidate.conflicts.length > 0 && " · metadata differs"}{candidate.related_versions.length > 0 && " · related version"}{candidate.possible_duplicates.length > 0 && " · possible duplicate"}</p>
            </div>;
          }}</VirtualArticleList>
          {!articles.length && <p className="p-4 text-sm text-muted-foreground">{discovery.session?.phase === "running" ? "Waiting for source results…" : discovery.session ? "No candidates obtained. Check source status and query details." : "Search results will appear here."}</p>}
          {discovery.nextOffset !== null && <Button className="m-2" variant="outline" disabled={discovery.loadingResults || discovery.updatesPending} onClick={discovery.more}>Load more obtained candidates</Button>}
        </div>
        <div className="max-h-[45%] space-y-2 overflow-auto border-t p-2 text-xs">
          <div className="flex flex-wrap items-center gap-2"><span>{selections.length} selected</span><Button size="xs" variant="ghost" onClick={() => setSelections([])}>Clear selection</Button><select aria-label="Import target collection" className="max-w-60 rounded border bg-background p-1" value={collection} onChange={event => setCollection(event.target.value)}><option value="">My library · unfiled</option>{collections.filter(item => !item.deleted).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></div>
          <div className="flex flex-wrap items-center gap-2"><label className="flex gap-1"><input type="checkbox" checked={fetchFulltext} onChange={event => setFetchFulltext(event.target.checked)} />Fetch available full text</label><Button size="xs" disabled={!selections.length || busy || !sessionId} onClick={() => void importAction(false)}>Preview import</Button></div>
          {preview && <div className="space-y-2 rounded border p-2" aria-label="Import preview"><p>{preview.editable ? "Target is writable. Existing metadata is preserved." : "Target library is read only."}</p>{preview.items.map(item => <div key={item.candidate_id}><span>{item.existing.length ? `Already in library: ${item.existing.map(article => article.title).join("; ")}` : "New library record"}</span>{item.related_existing.length > 0 && <p>Related version: {item.related_existing.map(article => article.title).join("; ")}</p>}{item.differences.length > 0 && <details><summary>Metadata differences</summary><pre className="whitespace-pre-wrap break-all">{pretty(item.differences)}</pre></details>}{item.error && <p className="text-destructive">{item.error}</p>}</div>)}<Button size="xs" disabled={busy || !preview.editable || preview.items.every(item => Boolean(item.error))} onClick={() => void importAction(true)}>Confirm import</Button></div>}
          {outcome && <div role="status"><p>{outcome.imported} imported · {outcome.items.filter(item => item.status === "already_present").length} already present · {outcome.failed} failed</p>{outcome.items.filter(item => item.error).map(item => <p key={item.candidate_id} className="text-destructive">{item.error}</p>)}</div>}
          {acquisition && <details open><summary>Full-text acquisition</summary>{acquisition.items.map(item => <p key={item.article_id}>{outcome?.items.find(result => result.article?.id === item.article_id)?.article?.title ?? item.article_id}: {item.status}{item.error && ` · ${item.error}`}</p>)}{!acquisition.completed_at && <Button size="xs" variant="outline" onClick={() => { void gatewayRequest("POST", `/bib/search/acquisitions/${encodeURIComponent(acquisition.id)}/cancel`).then(response => setAcquisition(response.acquisition)).catch(failure => setError(String(failure))); }}>Stop queued downloads</Button>}</details>}
        </div>
      </div>
      <aside aria-label="Discovery candidate details" className={`${detailsOpen ? "absolute inset-0 z-10 block lg:static" : "hidden lg:block"} overflow-auto rounded border bg-background p-3 text-sm`}>
        <Button size="xs" variant="outline" className="mb-2 lg:hidden" onClick={() => setDetailsOpen(false)}>Close candidate details</Button>
        {selected && article ? <div className="space-y-3"><h2 className="font-semibold">{article.title}</h2><label className="block text-xs">Metadata version<select aria-label="Candidate source version" className="mt-1 block w-full rounded border bg-background p-2" value={defaultVariant(selected)} onChange={event => { const index = Number(event.target.value); setVariants(current => ({ ...current, [selected.id]: index })); setSelections(current => current.map(item => item.candidate_id === selected.id ? { candidate_id: selected.id, expected_fingerprint: selected.fingerprint, record_index: index } : item)); }}>{selected.records.map((record, index) => <option key={index} value={index}>{record.source} · {record.article.title}</option>)}</select></label><p className="text-xs">{(article.authors ?? []).map(author => author.name ?? [author.fore_name, author.last_name].filter(Boolean).join(" ")).join("; ")}</p><p className="whitespace-pre-wrap text-xs">{article.abstract_text || "No abstract available."}</p>{article.identifiers?.map(identifier => <p className="break-all text-xs" key={`${identifier.kind}:${identifier.value}`}>{identifier.kind}: {identifierLink(identifier.kind, identifier.value) ? <a className="underline" href={identifierLink(identifier.kind, identifier.value)!} target="_blank" rel="noopener noreferrer">{identifier.value}</a> : identifier.value}</p>)}{selected.related_versions.length > 0 && <p className="text-xs">A related publication version is kept separately. Review before importing both.</p>}{selected.possible_duplicates.length > 0 && <p className="text-xs">Similar titles may be duplicates. They have not been merged automatically.</p>}{selected.conflicts.length > 0 && <details><summary>Source metadata differences</summary><pre className="whitespace-pre-wrap break-all text-xs">{pretty(selected.conflicts)}</pre></details>}<details><summary>Retrieval provenance</summary><pre className="whitespace-pre-wrap break-all text-xs">{pretty(selected.records.map(record => ({ source: record.source, rank: record.source_rank, retrieved_at: record.retrieved_at, executed_parameters: record.executed_parameters, translation: record.query_translation })))}</pre></details></div> : <p className="text-muted-foreground">Select a candidate to compare source metadata, publication versions and retrieval details.</p>}
      </aside>
    </div>
  </section>;
}
