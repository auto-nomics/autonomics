import { useEffect, useRef, useState } from "react";
import { gatewayRequest, newGatewayRequestId, saveTextFile } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import type { SearchRequest } from "./bib-search-model";
import type { DiscoveryOrder } from "./discovery-model";
import type { BibArticle } from "@/types";

export type SnapshotExecution = { kind: "library"; search: SearchRequest } | { kind: "discovery"; session_id: string; sequence: number; order: DiscoveryOrder };
type Summary = { id: string; name: string; kind: string; created_at: string; checksum: string; payload_bytes: number; coverage: { result_basis: string; complete_obtained_set: boolean; sources_exhausted: boolean | null; captured_items: number } };
type FrozenPage = { request: SearchRequest; response: { hits: { article_id: string }[]; articles: BibArticle[]; index_generation: string } };
type Capture = { started_at: string; finished_at: string; runtime: unknown; data: { kind: "library"; pages: FrozenPage[]; termination: string } | { kind: "discovery"; session: { request: unknown; sources: unknown[] }; results: { sequence: number; candidates: { article: BibArticle }[] }; termination: string } };
type Stored = { summary: Summary; capture: Capture };

export function SaveSnapshotButton({ execution, disabled }: { execution: SnapshotExecution; disabled?: boolean }) {
  const [draft, setDraft] = useState<SnapshotExecution | null>(null);
  const [name, setName] = useState("");
  const [maxItems, setMaxItems] = useState(1000);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState<Summary | null>(null);
  const [error, setError] = useState("");
  const action = useRef<{ fingerprint: string; key: string } | null>(null);
  const operation = useRef<AbortController | null>(null);
  const sequence = useRef(0);
  useEffect(() => () => { sequence.current++; operation.current?.abort(); }, []);
  function close() { sequence.current++; operation.current?.abort(); setDraft(null); setBusy(false); }
  function open() {
    const copy = structuredClone(execution);
    if (copy.kind === "library") copy.search.cursor = null;
    action.current = null; setSaved(null); setError(""); setDraft(copy); setMaxItems(1000);
    setName(`Search results ${new Date().toLocaleDateString()}`);
  }
  async function save() {
    if (!draft || !name.trim() || !Number.isInteger(maxItems) || maxItems < 1 || maxItems > 10000) return;
    const fingerprint = JSON.stringify({ name: name.trim(), max_items: maxItems, execution: draft });
    if (!action.current || action.current.fingerprint !== fingerprint) action.current = { fingerprint, key: newGatewayRequestId() };
    operation.current?.abort(); const abort = new AbortController(); operation.current = abort;
    const current = ++sequence.current; setBusy(true); setError("");
    try {
      const response: { snapshot: Stored } = await gatewayRequest("POST", "/bib/search/snapshots", { key: action.current.key, name: name.trim(), max_items: maxItems, deadline_ms: 120000, execution: draft }, { signal: abort.signal });
      if (current === sequence.current) setSaved(response.snapshot.summary);
    } catch (failure) { if (current === sequence.current && !abort.signal.aborted) setError(String(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }
  return <>
    <Button size="xs" variant="ghost" disabled={disabled} onClick={open}>Save result snapshot</Button>
    {draft && <SearchDialog title="Save result snapshot" onClose={close}>
      {saved ? <>
        <p role="status">Saved “{saved.name}”: {saved.coverage.captured_items} articles.</p>
        <p className="mt-2 text-xs text-muted-foreground">{saved.coverage.complete_obtained_set ? "All obtained results were captured." : "The saved set is partial; review its coverage in Snapshots."}</p>
        {saved.coverage.sources_exhausted === false && <p className="mt-2 text-xs">Some sources have more results or did not finish successfully.</p>}
        <p className="mt-2 text-xs">Open the Snapshots workspace to review or export this record.</p>
        <div className="mt-3 flex justify-end"><Button size="sm" onClick={close}>Done</Button></div>
      </> : <>
        <label className="block text-xs">Snapshot name<Input autoFocus aria-label="Snapshot name" maxLength={200} value={name} disabled={busy} onChange={event => setName(event.target.value)} /></label>
        <label className="mt-3 block text-xs">Maximum articles<Input aria-label="Snapshot maximum articles" type="number" min={1} max={10000} value={maxItems} disabled={busy} onChange={event => setMaxItems(Number(event.target.value))} /></label>
        <p className="mt-2 text-xs text-muted-foreground">Freeze this execution and its metadata. Saving a library search runs it again; changed results may differ from the current list. Saving discovery captures the current obtained candidates, including source failures.</p>
        <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{JSON.stringify(draft, null, 2)}</pre>
        {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
        <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={close}>Cancel</Button><Button size="sm" disabled={busy || !name.trim() || !Number.isInteger(maxItems) || maxItems < 1 || maxItems > 10000} onClick={() => void save()}>{busy ? "Capturing…" : error ? "Retry saving snapshot" : "Save snapshot"}</Button></div>
      </>}
    </SearchDialog>}
  </>;
}

function articlesOf(capture: Capture): BibArticle[] {
  if (capture.data.kind === "discovery") return capture.data.results.candidates.map(candidate => candidate.article);
  return capture.data.pages.flatMap(page => {
    const byId = new Map(page.response.articles.map(article => [article.id, article]));
    return page.response.hits.flatMap(hit => { const article = byId.get(hit.article_id); return article ? [article] : []; });
  });
}
export function SearchSnapshots({ active = true }: { active?: boolean }) {
  const [snapshots, setSnapshots] = useState<Summary[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [selected, setSelected] = useState<Stored | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [format, setFormat] = useState("bibtex");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [showDetails, setShowDetails] = useState(false);
  const operation = useRef<AbortController | null>(null);
  const sequence = useRef(0);
  function begin() { operation.current?.abort(); const abort = new AbortController(); operation.current = abort; return { abort, current: ++sequence.current }; }
  async function list(append = false) {
    const { abort, current } = begin(); setBusy(true); setError("");
    try {
      const response: { snapshots: Summary[]; next_cursor: string | null } = await gatewayRequest("GET", `/bib/search/snapshots?limit=50${append && next ? `&cursor=${encodeURIComponent(next)}` : ""}`, undefined, { signal: abort.signal });
      if (current === sequence.current) { setSnapshots(old => append ? [...old, ...response.snapshots] : response.snapshots); setNext(response.next_cursor); }
    } catch (failure) { if (current === sequence.current && !abort.signal.aborted) setError(String(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }
  useEffect(() => { if (active) void list(); return () => { sequence.current++; operation.current?.abort(); setBusy(false); }; }, [active]);
  async function open(id: string) {
    const { abort, current } = begin(); setBusy(true); setError(""); setSelected(null); setConfirmDelete(false); setShowDetails(false);
    try { const response = await gatewayRequest("GET", `/bib/search/snapshots/${encodeURIComponent(id)}`, undefined, { signal: abort.signal }); if (current === sequence.current) setSelected(response.snapshot); }
    catch (failure) { if (current === sequence.current && !abort.signal.aborted) setError(String(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }
  async function download() {
    if (!selected) return;
    const { abort, current } = begin(); setBusy(true); setError("");
    try {
      const response = await gatewayRequest("POST", `/bib/search/snapshots/${encodeURIComponent(selected.summary.id)}/export`, { format }, { signal: abort.signal });
      if (current !== sequence.current) return;
      await saveTextFile(response.file_name, response.content, response.content_type);
    } catch (failure) { if (current === sequence.current && !abort.signal.aborted) setError(String(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }
  async function remove() {
    if (!selected) return;
    const { abort, current } = begin(); setBusy(true); setError("");
    try {
      await gatewayRequest("DELETE", `/bib/search/snapshots/${encodeURIComponent(selected.summary.id)}?expected_checksum=${encodeURIComponent(selected.summary.checksum)}`, undefined, { signal: abort.signal });
      if (current !== sequence.current) return;
      setSelected(null); setConfirmDelete(false); await list();
    } catch (failure) { if (current === sequence.current && !abort.signal.aborted) setError(String(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }
  const articles = selected ? articlesOf(selected.capture) : [];
  const details = selected ? selected.capture.data.kind === "library" ? { request: selected.capture.data.pages[0]?.request, index_generations: [...new Set(selected.capture.data.pages.map(page => page.response.index_generation))] } : { request: selected.capture.data.session.request, sources: selected.capture.data.session.sources, sequence: selected.capture.data.results.sequence } : null;
  return <section aria-label="Search snapshots" className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3 text-sm">
    <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="font-medium">Result snapshots</h2><Button size="sm" variant="outline" disabled={busy} onClick={() => void list()}>Refresh snapshots</Button></div>
    <p className="text-xs text-muted-foreground">Fixed results from an execution. Saved searches remain dynamic rules.</p>
    {error && <p role="alert" className="text-destructive">{error}</p>}{busy && <p role="status">Loading snapshot data…</p>}
    <div className="grid min-h-0 gap-3 md:grid-cols-[minmax(200px,30%)_minmax(0,1fr)]">
      <div className="min-w-0 space-y-2" aria-label="Snapshot list">
        {!snapshots.length && !busy && <p>No snapshots saved.</p>}
        {snapshots.map(snapshot => <button className="block w-full rounded border p-2 text-left hover:bg-muted" key={snapshot.id} aria-pressed={selected?.summary.id === snapshot.id} onClick={() => void open(snapshot.id)}><span className="block break-words font-medium">{snapshot.name}</span><span className="text-xs text-muted-foreground">{snapshot.coverage.captured_items} articles · {new Date(snapshot.created_at).toLocaleString()} · {snapshot.coverage.complete_obtained_set ? "complete obtained set" : "partial set"}</span></button>)}
        {next && <Button size="sm" variant="outline" disabled={busy} onClick={() => void list(true)}>Load more snapshots</Button>}
      </div>
      {selected && <article aria-label="Frozen snapshot preview" className="min-w-0 space-y-3 rounded border p-3">
        <h3 className="break-words font-medium">{selected.summary.name}</h3>
        <p className="text-xs">{selected.summary.coverage.captured_items} saved articles · {selected.summary.coverage.result_basis.replaceAll("_", " ")} · {selected.summary.coverage.complete_obtained_set ? "complete obtained set" : "partial set"}</p>
        {selected.summary.coverage.sources_exhausted === false && <p className="text-xs">Some sources have more results or failed. Review execution details for their saved state.</p>}
        <p className="text-xs text-muted-foreground">Metadata and excerpts are frozen. Original files are not included.</p>
        <div className="flex flex-wrap gap-2"><select aria-label="Snapshot export format" className="rounded border bg-background p-1" value={format} onChange={event => setFormat(event.target.value)}><option value="bibtex">BibTeX</option><option value="ris">RIS</option><option value="json">JSON with execution details</option></select><Button size="xs" variant="outline" disabled={busy} onClick={() => void download()}>Export snapshot</Button><Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirmDelete(true)}>Delete snapshot</Button></div>
        {confirmDelete && <div className="rounded border p-2"><p>Delete “{selected.summary.name}”?</p><Button size="xs" variant="ghost" onClick={() => setConfirmDelete(false)}>Keep snapshot</Button><Button size="xs" disabled={busy} onClick={() => void remove()}>Confirm snapshot deletion</Button></div>}
        <Button size="xs" variant="ghost" aria-expanded={showDetails} onClick={() => setShowDetails(!showDetails)}>Execution details</Button>
        {showDetails && <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{JSON.stringify({ started_at: selected.capture.started_at, finished_at: selected.capture.finished_at, coverage: selected.summary.coverage, runtime: selected.capture.runtime, ...details }, null, 2)}</pre>}
        {articles.length > 100 && <p className="text-xs">Previewing the first 100 of {articles.length} saved articles. Export includes the full saved set.</p>}
        <ol className="space-y-2">{articles.slice(0,100).map((article,index) => <li key={`${index}:${article.id}`} className="rounded border p-2"><p className="break-words font-medium">{article.title}</p>{article.year && <p className="text-xs">{article.year}</p>}{article.abstract_text && <p className="mt-1 whitespace-pre-wrap break-words text-xs">{article.abstract_text}</p>}</li>)}</ol>
      </article>}
    </div>
  </section>;
}
