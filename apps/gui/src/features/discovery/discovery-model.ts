import type { BibArticle } from "@/types";
import type { SearchExpr, SearchField } from "@/features/bibliography";

export type DiscoveryRequest = {
  query?: string; expression?: SearchExpr | null;
  native?: { source: string; parameters: Record<string, string> };
  sources: string[];
  filters?: { year_from?: number; year_to?: number; publication_types?: string[]; open_access?: boolean };
  sort?: "relevance" | "newest" | "oldest" | "citation_count";
  page_size?: number;
  allow_approximate?: boolean;
};
export type SourceNotice = { code: string; message: string };
export type SourceCapabilities = {
  source: string; searchable: boolean; native_query: boolean; fields: SearchField[];
  filters: string[]; sorts: string[]; pagination: string; max_page_size: number;
  credentials_configured: boolean; notes: string[];
  authentication?: "none" | "optional" | "required" | "required_for_continuation" | "unknown";
  quota: { remaining: number | null; limit: number | null; observed_at_unix_seconds: number } | null;
};
export type DiscoverySourceStatus = {
  source: string; phase: string; total: number | null; delivered_records: number; pages: number; has_more: boolean;
  failure: { kind: string; message: string; retryable: boolean; retry_after_seconds: number | null } | null;
  plan: { parameters: Record<string, string>; fingerprint: string; notices: SourceNotice[] } | null;
  executed_parameters: Record<string, string>; query_translation: string | null; notices: SourceNotice[];
};
export type DiscoverySession = {
  id: string; sequence: number; phase: "running" | "idle" | "cancelled";
  request: DiscoveryRequest; merged_candidates: number; expires_at: string; sources: DiscoverySourceStatus[];
};
export type DiscoveryRecord = {
  source: string; article: BibArticle; source_rank: number; retrieved_at: string;
  executed_parameters: Record<string, string>; query_translation: string | null;
};
export type DiscoveryCandidate = {
  id: string; fingerprint: string; article: BibArticle; records: DiscoveryRecord[]; first_seen: number; rrf_score: number;
  conflicts: Array<{ field: string; variants: Array<{ source: string; value: unknown }> }>;
  related_versions: Array<{ candidate_id: string; reason: string }>; possible_duplicates: string[];
};
export type DiscoveryResultPage = { candidates: DiscoveryCandidate[]; obtained_candidates: number; next_offset: number | null; sequence: number };
export type DiscoverySelection = { candidate_id: string; expected_fingerprint: string; record_index: number };
export type DiscoveryImportRequest = { selections: DiscoverySelection[]; collection_id?: string; library_id?: number; fetch_fulltext?: boolean };
export type DiscoveryPreview = {
  library_id: number; collection_id: string | null; editable: boolean;
  items: Array<{ candidate_id: string; existing: BibArticle[]; related_existing: BibArticle[]; differences: DiscoveryCandidate["conflicts"]; error: string | null }>;
};
export type DiscoveryAcquisition = {
  id: string; completed_at: string | null; cancel_requested: boolean;
  items: Array<{ article_id: string; status: string; error: string | null }>;
};
export type DiscoveryImportResult = {
  imported: number; failed: number;
  items: Array<{ candidate_id: string; status: string; article: BibArticle | null; error: string | null }>;
  acquisition: DiscoveryAcquisition | null;
};
export type DiscoveryOrder = "first_seen" | "relevance" | "newest" | "source_rank";
