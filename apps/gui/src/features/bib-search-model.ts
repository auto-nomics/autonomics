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
export type SearchRequest = { query?: string; expression?: SearchExpr | null; default_fields?: SearchField[]; scope?: SearchScope; filters?: SearchFilters; sort?: SearchSort; page_size?: number; cursor?: string | null; include_facets?: boolean; include_articles?: boolean };
export type HighlightRange = { start: number; end: number };
export type SearchHit = { article_id: string; title: string; score: number; snippet: string; anchor_key?: string; matched_field?: string; highlight_ranges: HighlightRange[] };
export type FacetValue = { value: string; label: string; count: number };
export type SearchFacets = { tags: FacetValue[]; creators: FacetValue[]; publication_types: FacetValue[]; years: FacetValue[]; fulltext_status: FacetValue[] };
export type SearchPage = { hits: SearchHit[]; articles: BibArticle[]; total: number; next_cursor?: string | null; index_generation: string; facets?: SearchFacets; attachment_summaries?: Record<string, any> };
export type SavedSearch = { id: string; name: string; version: number; schema_version: number; request: SearchRequest; created_at: string; updated_at: string };
export type SearchEvidence = { article_id: string; item_key: string; attachment_key?: string | null; attachment_hash?: string | null; annotation_key?: string | null; annotation_id?: string | null; matched_field: string; snippet: string; highlight_ranges: HighlightRange[]; matched_query: string; page?: number | null; content_revision: string; location_status: "metadata" | "annotation" | "text_search" };
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
