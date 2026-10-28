import type { BibArticle, BibCollection } from "@/types";

export type SearchField = "any" | "identifier" | "title" | "creator" | "abstract" | "publication" | "year" | "keyword" | "tag" | "note" | "attachment_content";
export type SearchExpr =
  | { op: "term" | "phrase"; value: string; field?: SearchField | null }
  | { op: "and" | "or"; clauses: SearchExpr[] }
  | { op: "not"; clause: SearchExpr }
  | { op: "range"; field: SearchField; gte?: string | null; lte?: string | null };
export type SearchFilters = {
  tag_ids?: number[];
  creator_ids?: number[];
  publication_types?: string[];
  year_from?: number | null;
  year_to?: number | null;
  has_fulltext?: boolean | null;
  has_annotations?: boolean | null;
  fulltext_status?: "indexed" | "pending" | "failed" | "missing" | null;
};
export type SearchSort = "relevance" | "newest" | "oldest" | "title";
export type SearchScope = { library_id?: number | null; collection_id?: string | null; include_descendants?: boolean; unfiled?: boolean; require_same_child?: boolean };
export type SearchMode = "lexical" | "hybrid";
export type SearchRequest = { query?: string; expression?: SearchExpr | null; default_fields?: SearchField[]; scope?: SearchScope; filters?: SearchFilters; sort?: SearchSort; page_size?: number; cursor?: string | null; include_facets?: boolean; include_articles?: boolean; mode?: SearchMode; semantic_query?: string | null };
export type SemanticEvidence = { doc_id: string; anchor_key: string; kind: string; matched_field: string; content_hash: string; start_byte: number; end_byte: number; snippet: string };
export type SearchRecall = { lexical_rank: number | null; semantic_rank: number | null; semantic_evidence?: SemanticEvidence | null };
export type SearchRetrieval = { algorithm: string; total_kind: "candidate_pool"; facets_basis: string; eligible_articles: number; candidate_depth: number; semantic_generation: string; configuration_generation: string; indexed_chunks: number; hits: Record<string, SearchRecall> };
export type SemanticConfig = { enabled: boolean; origin: string; allow_remote: boolean; model: string; model_digest: string; profile: { dimensions: number; query_instruction: string; chunk_chars: number | null; overlap_chars: number; include_annotations: boolean; include_fulltext: boolean }; batch_size: number; inference?: { num_ctx?: number | null; num_batch?: number | null }; limits?: SemanticLimits; timeout_seconds: number };
export type SemanticProgress = { stage: string; target_revision: number | null; target_articles: number | null; processed_articles: number; processed_chunks: number; embedded_chunks: number; cached_chunks: number };
export type SemanticLimits = { max_chunks: number; max_input_bytes: number; max_vector_bytes: number; max_disk_bytes: number };
export type SemanticMaintenanceAction = "rebuild" | "repair" | "resume" | "cancel";
export type SemanticMaintenance = { request_id: string; action: SemanticMaintenanceAction; configuration_generation: string; state: string; progress: SemanticProgress | null; last_error: string | null; store_generation: string | null };
export type SemanticStatus = { config: SemanticConfig; configuration_generation: string; phase: "disabled" | "loading" | "indexing" | "ready" | "failed" | "paused"; generation: string | null; signature: string | null; indexed_revision: number | null; indexed_chunks: number; resources?: { vector_bytes: number; disk_bytes: number | null } | null; last_error: string | null; progress?: SemanticProgress | null; maintenance?: SemanticMaintenance | null; store_generation?: string };
export type HighlightRange = { start: number; end: number };
export type SearchHit = { article_id: string; title: string; score: number; snippet: string; anchor_key?: string; matched_field?: string; highlight_ranges: HighlightRange[] };
export type FacetValue = { value: string; label: string; count: number };
export type SearchFacets = { tags: FacetValue[]; creators: FacetValue[]; publication_types: FacetValue[]; years: FacetValue[]; fulltext_status: FacetValue[] };
export type SearchPage = { hits: SearchHit[]; articles: BibArticle[]; total: number; next_cursor?: string | null; index_generation: string; facets?: SearchFacets; attachment_summaries?: Record<string, any>; retrieval?: SearchRetrieval | null };
export type SavedSearch = { id: string; name: string; version: number; schema_version: number; request: SearchRequest; created_at: string; updated_at: string };
export type SearchEvidence = { article_id: string; item_key: string; attachment_key?: string | null; attachment_hash?: string | null; annotation_key?: string | null; annotation_id?: string | null; matched_field: string; snippet: string; highlight_ranges: HighlightRange[]; matched_query: string; page?: number | null; content_revision: string; location_status: "metadata" | "annotation" | "text_search"; semantic_evidence?: SemanticEvidence | null };
export type EvidencePage = { evidence: SearchEvidence[]; total: number; next_cursor?: string | null; index_generation: string };
export type IndexStatus = { engine: string; source_revision: number; indexed_revision?: number; indexed_documents: number; pending_articles: number; building: boolean; processed_documents: number; target_revision?: number; last_error?: string | null; durable: boolean };
export type LibraryMetadata = { collections: BibCollection[]; total: number };

export function hasFilters(filters: SearchFilters): boolean {
  return Object.values(filters).some(value => Array.isArray(value) ? value.length > 0 : value !== null && value !== undefined);
}

export function formatExpression(expr: SearchExpr): string {
  const fields: Partial<Record<SearchField, string>> = { creator: "author", publication: "journal", attachment_content: "fulltext", note: "note" };
  switch (expr.op) {
    case "and": case "or": return `(${expr.clauses.map(formatExpression).join(` ${expr.op.toUpperCase()} `)})`;
    case "not": return `NOT (${formatExpression(expr.clause)})`;
    case "range": return `year:${expr.gte ?? ""}..${expr.lte ?? ""}`;
    default: {
      const field = expr.field && expr.field !== "any" ? `${fields[expr.field] ?? expr.field}:` : "";
      const value = expr.op === "phrase" || /[\s()"]/.test(expr.value) || /^(AND|OR|NOT)$/.test(expr.value) ? JSON.stringify(expr.value) : expr.value;
      return `${field}${value}`;
    }
  }
}


export const searchFieldLabels: Record<SearchField, string> = {any:"Any field",identifier:"Identifier",title:"Title",creator:"Author",abstract:"Abstract",publication:"Journal",year:"Year",keyword:"Keyword",tag:"Tag text",note:"Notes",attachment_content:"Attachment text"};
export const quickFields: Record<string, SearchField[]> = {
  all: [],
  title: ["title", "creator", "publication", "year"],
  metadata: ["title", "creator", "publication", "identifier", "abstract", "keyword", "tag", "note", "year"],
};

export function builderIsLossless(expr: SearchExpr): boolean {
  switch (expr.op) {
    case "and": case "or": return expr.clauses.length > 0 && expr.clauses.every(builderIsLossless);
    case "not": return builderIsLossless(expr.clause);
    case "term": return !/\s/.test(expr.value);
    case "range": return expr.field === "year";
    default: return true;
  }
}
