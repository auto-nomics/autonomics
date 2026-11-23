import { AddToContext } from "@/features/context/context-basket";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchSnippet } from "./search-snippet";
import type { EvidencePage, SearchEvidence, SearchRequest } from "./bib-search-model";

const fieldLabel: Record<string,string> = {title:"Title",abstract:"Abstract",creator:"Author",publication:"Journal",identifier:"Identifier",keyword:"Keyword",tag:"Tag",year_text:"Year",annotation:"Annotation",fulltext:"Attachment text"};
export function SearchEvidencePanel({ articleId, request, generation, onOpen }: { articleId: string; request: SearchRequest; generation: string; onOpen: (evidence: SearchEvidence) => void }) {
  const { t } = useTranslation("bibliography");
  const field = (key: string) => t(`searchEvidence.fields.${key}`, { defaultValue: fieldLabel[key] ?? key });
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
      if (!Array.isArray(page.evidence) || typeof page.total !== "number") throw new Error(t("searchEvidence.unavailable"));
      if (id !== sequence.current || context !== latest.current) return;
      setState(current => ({ context, page: { ...page, evidence: cursor ? [...current.page.evidence, ...page.evidence] : page.evidence }, loading: false, error: "" }));
    } catch (failure) { if (id === sequence.current && context === latest.current && !abort.current?.signal.aborted) setState(current => ({ ...current, loading: false, error: String(failure) })); }
  };
  useEffect(() => { void load(); return () => { sequence.current++; abort.current?.abort(); }; }, [context]);
  const current = state.context === context;
  return <section aria-label={t("searchEvidence.sectionAria")} className="space-y-2 rounded border p-2 text-xs">
    <div className="flex items-center justify-between"><span className="font-semibold">{t("searchEvidence.heading")}</span>{current && !state.loading && <span>{t("searchEvidence.objects", { count: state.page.total })}</span>}</div>
    {(!current || state.loading) && <p role="status" className="text-muted-foreground">{t("searchEvidence.finding")}</p>}
    {current && state.error && <p role="alert" className="text-destructive">{state.error}<Button size="xs" variant="ghost" onClick={() => void load()}>{t("searchEvidence.retry")}</Button></p>}
    {current && !state.loading && !state.error && !state.page.total && <p className="text-muted-foreground">{t("searchEvidence.noLocation")}</p>}
    {current && state.page.evidence.map((evidence, index) => <div key={`${evidence.attachment_key ?? evidence.annotation_key ?? evidence.item_key}-${index}`} className="space-y-1 border-t pt-2">
      <p className="font-medium">{evidence.semantic_evidence ? t("searchEvidence.relatedEvidence") : ""}{evidence.matched_field in fieldLabel ? field(evidence.matched_field) : t("searchEvidence.match")}{evidence.page ? t("searchEvidence.pageSuffix", { page: evidence.page }) : ""}{evidence.annotation_key ? t("searchEvidence.annotationSuffix") : ""}</p>
      <p className="break-words"><SearchSnippet text={evidence.snippet} ranges={evidence.highlight_ranges} /></p>
      {evidence.semantic_evidence && evidence.semantic_evidence.snippet !== evidence.snippet && <p className="break-words text-muted-foreground">{t("searchEvidence.relatedPassage")}{evidence.semantic_evidence.snippet}</p>}
      {evidence.semantic_evidence && !evidence.highlight_ranges.length && <p className="text-muted-foreground">{t("searchEvidence.semanticNote")}</p>}
      {evidence.attachment_key || evidence.annotation_id ? <Button size="xs" variant="secondary" onClick={() => onOpen(evidence)}>{evidence.attachment_key ? t("searchEvidence.openEvidence") : t("searchEvidence.showNote")}</Button> : <p className="text-muted-foreground">{t("searchEvidence.shownInDetails")}</p>}
      <AddToContext resource={{ kind: "evidence", id: JSON.stringify([articleId, evidence.item_key, evidence.attachment_key, evidence.annotation_key, evidence.matched_field, evidence.page, evidence.snippet]), title: `${evidence.matched_field in fieldLabel ? field(evidence.matched_field) : t("searchEvidence.match")}${evidence.page ? t("searchEvidence.pageSuffix", { page: evidence.page }) : ""}`, scope: "检索命中片段（非全文）", source: `bib:${articleId} · ${evidence.attachment_key ?? evidence.annotation_key ?? evidence.item_key}${evidence.page ? ` · 第 ${evidence.page} 页` : ""}`, revision: evidence.content_revision, content: JSON.stringify(evidence, null, 2) }} />
      {evidence.location_status === "text_search" && <p className="text-muted-foreground">{t("searchEvidence.opensAttachment")}</p>}
    </div>)}
    {current && state.page.next_cursor && <Button size="xs" variant="ghost" disabled={state.loading} onClick={() => void load(state.page.next_cursor)}>{t("searchEvidence.more")}</Button>}
  </section>;
}
