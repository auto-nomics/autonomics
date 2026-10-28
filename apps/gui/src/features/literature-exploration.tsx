import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { BibArticle } from "@/types";
import { ExplorationImportReview, type ExplorationImportTarget } from "./exploration-import";

type Tab = "references" | "citations" | "similar";
type Pool = "recent" | "all-cs";
type Item = { position: number; article: BibArticle | null; unresolved: Record<string, unknown> | null; citation: { citing_paper_id: string; cited_paper_id: string } | null; contexts: string[] | null; intents: string[] | null; is_influential: boolean | null };
type Page = { seed: string; items: Item[]; unresolved_records: number; next_cursor: string | null; retrieved_at: string; upstream_snapshot: boolean };
const labels: Record<Tab, string> = { references: "References", citations: "Cited by", similar: "Similar papers" };
function seedIdentifier(article: BibArticle) {
  for (const kind of ["s2", "doi", "pmid", "arxiv"]) {
    const id = article.identifiers?.find(id => id.kind.toLowerCase() === kind && id.value.trim());
    if (id) return kind === "s2" ? id.value : `${kind.toUpperCase()}:${id.value}`;
  }
  return null;
}
function sourceLink(article: BibArticle | null) {
  const id = article?.identifiers?.find(id => id.kind.toLowerCase() === "s2" && /^[a-f\d]{40}$/i.test(id.value));
  return id ? `https://www.semanticscholar.org/paper/${encodeURIComponent(id.value)}` : null;
}

export function LiteratureExploration({ article, importTarget = { library_id: 1 }, targetLabel = "Library", onImported }: { article: BibArticle; importTarget?: ExplorationImportTarget; targetLabel?: string; onImported?: () => void }) {
  const seed = seedIdentifier(article);
  return <Explorer key={seed ?? article.id} seed={seed} importTarget={importTarget} targetLabel={targetLabel} onImported={onImported} />;
}
function Explorer({ seed, importTarget, targetLabel, onImported }: { seed: string | null; importTarget: ExplorationImportTarget; targetLabel: string; onImported?: () => void }) {
  const [importId, setImportId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("references");
  const [pool, setPool] = useState<Pool>("recent");
  const [pages, setPages] = useState<Record<string, Page>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const operation = useRef<AbortController | null>(null);
  const sequence = useRef(0);
  const viewport = useRef<HTMLDivElement>(null);
  const scroll = useRef<Record<string, number>>({});
  const cacheKey = tab === "similar" ? `similar:${pool}` : tab;
  const page = pages[cacheKey];
  useEffect(() => () => { sequence.current++; operation.current?.abort(); }, []);
  useEffect(() => { if (viewport.current) viewport.current.scrollTop = scroll.current[cacheKey] ?? 0; }, [cacheKey]);

  async function load(nextTab: Tab, nextPool: Pool, more = false) {
    if (!seed) return;
    const key = nextTab === "similar" ? `similar:${nextPool}` : nextTab;
    const previous = pages[key];
    if (more && !previous?.next_cursor) return;
    operation.current?.abort();
    const abort = new AbortController(); operation.current = abort;
    const current = ++sequence.current;
    setTab(nextTab); setPool(nextPool); setError(""); setBusy(true);
    try {
      const result: Page = await gatewayRequest("POST", "/bib/search/explore", {
        seed: more ? previous.seed : seed,
        kind: nextTab === "similar" ? { kind: "similar", pool: nextPool } : { kind: nextTab },
        page_size: 25, cursor: more ? previous.next_cursor : null, deadline_ms: 15000,
      }, { signal: abort.signal });
      if (current !== sequence.current) return;
      setPages(pages => ({ ...pages, [key]: more ? { ...result, items: [...previous.items, ...result.items], unresolved_records: previous.unresolved_records + result.unresolved_records } : result }));
    } catch (failure) {
      if (current === sequence.current && !abort.signal.aborted) setError(String(failure));
    } finally { if (current === sequence.current) setBusy(false); }
  }
  function choose(nextTab: Tab, nextPool = pool) {
    operation.current?.abort(); sequence.current++; setBusy(false); setError("");
    setTab(nextTab); setPool(nextPool);
    const key = nextTab === "similar" ? `similar:${nextPool}` : nextTab;
    if (!pages[key]) void load(nextTab, nextPool);
  }
  return <section className="space-y-2 rounded-md border p-2 text-xs" aria-label="Literature exploration">
    <h3 className="font-medium">Explore literature</h3>
    {importId && <ExplorationImportReview key={importId} paperId={importId} target={importTarget} targetLabel={targetLabel} onClose={() => setImportId(null)} onImported={onImported} />}
    {!seed ? <p className="text-muted-foreground">A DOI, PMID, arXiv or Semantic Scholar identifier is needed.</p> : <>
      <div className="flex flex-wrap gap-1" aria-label="Exploration lists">
        {(Object.keys(labels) as Tab[]).map(value => <Button key={value} size="xs" variant={tab === value ? "secondary" : "ghost"} aria-pressed={tab === value} onClick={() => choose(value)}>{labels[value]}</Button>)}
      </div>
      {tab === "similar" && <label className="flex items-center gap-2">Recommendation pool<select aria-label="Recommendation pool" className="min-w-0 rounded border bg-background p-1" value={pool} onChange={event => choose("similar", event.target.value as Pool)}><option value="recent">Recent papers</option><option value="all-cs">All computer science</option></select></label>}
      <p className="text-muted-foreground">Semantic Scholar · {tab === "similar" ? "Recommendations, not citation relationships." : "Live citation list; upstream changes may affect later pages."}</p>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      <div className="flex flex-wrap gap-2">
        <Button size="xs" variant="outline" disabled={busy} onClick={() => void load(tab, pool)}>{page ? "Refresh list" : "Load list"}</Button>
        {busy && <Button size="xs" variant="ghost" onClick={() => { operation.current?.abort(); sequence.current++; setBusy(false); }}>Cancel exploration</Button>}
      </div>
      <div ref={viewport} className="max-h-80 overflow-auto" onScroll={event => { scroll.current[cacheKey] = event.currentTarget.scrollTop; }} role="region" aria-label={labels[tab]} aria-busy={busy}>
        {busy && <p role="status">Loading {labels[tab].toLowerCase()}…</p>}
        {page && <>
          <p className="mb-2 text-muted-foreground">{page.items.length} obtained · {page.unresolved_records} unresolved · Retrieved {new Date(page.retrieved_at).toLocaleString()}</p>
          {!page.items.length && <p>No records returned by this source.</p>}
          <ol className="space-y-2">{page.items.map(item => {
            const href = sourceLink(item.article);
            const title = item.article?.title ?? (typeof item.unresolved?.title === "string" ? item.unresolved.title : "Unresolved reference");
            return <li key={item.position} className="rounded border p-2">
              <p className="font-medium">{href ? <a className="underline" href={href} target="_blank" rel="noreferrer">{title}</a> : title}</p>
              {item.article?.year && <p>{item.article.year}</p>}
              {href && <Button size="xs" variant="outline" onClick={() => setImportId(href.split("/").at(-1)!)}>Review import</Button>}
              {!item.article && <p className="text-muted-foreground">Source could not resolve this entry. It cannot be imported as a verified paper.</p>}
              {item.citation && <p className="text-muted-foreground">{tab === "references" ? "Referenced by the selected paper" : "Cites the selected paper"}</p>}
              {!!item.contexts?.length && <details><summary>Citation contexts</summary>{item.contexts.map((context,index) => <p key={index} className="mt-1 whitespace-pre-wrap">{context}</p>)}</details>}
            </li>;
          })}</ol>
          {page.next_cursor && <Button className="mt-2" size="xs" variant="outline" disabled={busy} onClick={() => void load(tab, pool, true)}>Load more {labels[tab].toLowerCase()}</Button>}
        </>}
      </div>
    </>}
  </section>;
}
