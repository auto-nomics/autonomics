import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchSnippet } from "./search-snippet";
import type { EvidencePage, SearchEvidence, SearchRequest } from "./bib-search-model";

const fieldLabel: Record<string,string> = {title:"Title",abstract:"Abstract",creator:"Author",publication:"Journal",identifier:"Identifier",keyword:"Keyword",tag:"Tag",year_text:"Year",annotation:"Annotation",fulltext:"Attachment text"};
export function SearchEvidencePanel({ articleId, request, generation, onOpen }: { articleId: string; request: SearchRequest; generation: string; onOpen: (evidence: SearchEvidence) => void }) {
  const context = JSON.stringify({ articleId, request, generation });
  const [state, setState] = useState<{ context: string; page: EvidencePage; loading: boolean; error: string }>({ context: "", page: { evidence: [], total: 0, index_generation: "" }, loading: false, error: "" });
  const sequence = useRef(0);
  const abort = useRef<AbortController | null>(null);
  const latest = useRef(context); latest.current = context;
  const load = async (cursor?: string | null) => {
    const id = ++sequence.current; abort.current?.abort(); abort.current = new AbortController();
    setState(current => ({ ...current, context, loading: true, error: "", page: cursor && current.context === context ? current.page : { evidence: [], total: 0, index_generation: "" } }));
    try {
      const page: EvidencePage = await gatewayRequest("POST", `/bib/articles/${encodeURIComponent(articleId)}/search-evidence`, {search: { ...request, cursor: cursor ?? null, page_size: 20 }, index_generation: generation || null}, { signal: abort.current.signal });
      if (!Array.isArray(page.evidence) || typeof page.total !== "number") throw new Error("Search evidence is unavailable from this server.");
      if (id !== sequence.current || context !== latest.current) return;
      setState(current => ({ context, page: { ...page, evidence: cursor ? [...current.page.evidence, ...page.evidence] : page.evidence }, loading: false, error: "" }));
    } catch (failure) { if (id === sequence.current && context === latest.current && !abort.current?.signal.aborted) setState(current => ({ ...current, loading: false, error: String(failure) })); }
  };
  useEffect(() => { void load(); return () => { sequence.current++; abort.current?.abort(); }; }, [context]);
  const current = state.context === context;
  return <section aria-label="Search evidence" className="space-y-2 rounded border p-2 text-xs">
    <div className="flex items-center justify-between"><span className="font-semibold">Search evidence</span>{current && !state.loading && <span>{state.page.total} objects</span>}</div>
    {(!current || state.loading) && <p role="status" className="text-muted-foreground">Finding evidence…</p>}
    {current && state.error && <p role="alert" className="text-destructive">{state.error}<Button size="xs" variant="ghost" onClick={() => void load()}>Retry evidence</Button></p>}
    {current && !state.loading && !state.error && !state.page.total && <p className="text-muted-foreground">The conditions match this article without a positive text location.</p>}
    {current && state.page.evidence.map((evidence, index) => <div key={`${evidence.attachment_key ?? evidence.annotation_key ?? evidence.item_key}-${index}`} className="space-y-1 border-t pt-2">
      <p className="font-medium">{evidence.semantic_evidence ? "Related evidence · " : ""}{fieldLabel[evidence.matched_field] ?? "Match"}{evidence.page ? ` · Page ${evidence.page}` : ""}{evidence.annotation_key ? " · Annotation" : ""}</p>
      <p className="break-words"><SearchSnippet text={evidence.snippet} ranges={evidence.highlight_ranges} /></p>
      {evidence.semantic_evidence && evidence.semantic_evidence.snippet !== evidence.snippet && <p className="break-words text-muted-foreground">Related passage: {evidence.semantic_evidence.snippet}</p>}
      {evidence.semantic_evidence && !evidence.highlight_ranges.length && <p className="text-muted-foreground">Selected by semantic similarity; no literal keyword match is implied.</p>}
      {evidence.attachment_key || evidence.annotation_id ? <Button size="xs" variant="secondary" onClick={() => onOpen(evidence)}>{evidence.attachment_key ? "Open evidence" : "Show note"}</Button> : <p className="text-muted-foreground">Shown in entry details.</p>}
      {evidence.location_status === "text_search" && <p className="text-muted-foreground">Opens the matching attachment and locates this text.</p>}
    </div>)}
    {current && state.page.next_cursor && <Button size="xs" variant="ghost" disabled={state.loading} onClick={() => void load(state.page.next_cursor)}>More evidence</Button>}
  </section>;
}
