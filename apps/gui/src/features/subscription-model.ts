import type { BibArticle } from "@/types";
import type { DiscoveryRecord, DiscoveryRequest } from "./discovery-model";
export type SubscriptionSpec = { name: string; request: DiscoveryRequest; enabled: boolean; interval_seconds: number; initial_days: number; overlap_days: number; window_days: number; max_pages_per_run: number; max_records_per_run: number; run_timeout_ms: number };
export const defaultSubscription = (request: DiscoveryRequest): SubscriptionSpec => ({ name: "Literature watch", request: structuredClone(request), enabled: false, interval_seconds: 3600, initial_days: 30, overlap_days: 3, window_days: 1, max_pages_per_run: 20, max_records_per_run: 1000, run_timeout_ms: 120000 });
export type SourceState = { source: string; watermark_ms: number | null; complete: boolean; cursor: string | null; last_checked_ms: number | null; last_error: { message: string; retryable: boolean } | null; retry_at_ms: number | null; window: unknown; plan: unknown; scan?: { strategy: string; done: boolean; target_ms: number | null } };
export type SubscriptionRule = { id: string; version: number; spec: SubscriptionSpec; next_due_ms: number; last_success_ms: number | null; sources: SourceState[]; resource_wait?: { since_ms: number; reason: string } | null };
export type InboxItem = { id: string; rule_id: string; rule_version: number; first_matched_ms: number; records: DiscoveryRecord[]; related_groups: string[]; read: boolean };
export type RunSummary = { id: string; rule_id: string; rule_version: number; started_ms: number; finished_ms: number | null; state: string; pages_started: number; pages_committed: number; records: number; new_notifications: number };
export type Run = RunSummary & { reason: string | null; spec: SubscriptionSpec; sources: { source: string; strategy: string; done: boolean; error: { message: string } | null; windows: unknown[] }[] };
export type SubscriptionLimits = { max_rules: number; max_seen: number; max_seen_bytes: number; max_inbox: number; max_inbox_bytes: number; max_page_bytes: number; max_checkpoints: number; max_runs: number; max_run_bytes: number; max_history_bytes: number };
export type NotificationLookup = { request: DiscoveryRequest; label: string; accepts: (article: BibArticle) => boolean };
function normalize(kind: string, value: string): string | null {
  let id = value.trim().toLowerCase();
  if (kind === "doi" || kind === "biorxiv") {
    id = id.replace(/^(?:https?:\/\/(?:dx\.)?doi\.org\/|doi:\s*)/, "").trim();
    return /^10\.\d{4,9}\/\S+$/.test(id) && !/[\u0000-\u001f\u007f]/.test(id) ? id : null;
  }
  if (kind === "pmid") {
    id = id.replace(/^(?:pmid:\s*|https?:\/\/pubmed\.ncbi\.nlm\.nih\.gov\/)/, "").split(/[?#]/)[0].replace(/\/$/, "");
    if (!/^\d+$/.test(id)) return null;
    try { const n = BigInt(id); return n > 0n && n <= 18446744073709551615n ? n.toString() : null; } catch { return null; }
  }
  if (kind === "openalex") { id = id.replace(/^(?:https?:\/\/openalex\.org\/|openalex:)/, "").split(/[?#]/)[0].replace(/\/$/, ""); return /^w\d+$/.test(id) ? id : null; }
  if (kind === "s2") return /^[a-f\d]{40}$/.test(id) ? id : null;
  if (kind === "arxiv") {
    id = id.replace(/^(?:https?:\/\/arxiv\.org\/(?:abs|pdf)\/|arxiv:\s*)/, "").split(/[?#]/)[0].replace(/\/$/, "").replace(/\.pdf$/, "");
    const base = id.replace(/v[1-9]\d*$/, "");
    const date = base.includes("/") ? base.split("/")[1] : base.split(".")[0];
    return /^(?:\d{4}\.\d{4,5}|[a-z][a-z\d.-]*\/\d{7})(?:v[1-9]\d*)?$/.test(id) && Number(date.slice(2, 4)) >= 1 && Number(date.slice(2, 4)) <= 12 ? id : null;
  }
  return null;
}
function values(article: BibArticle, kind: string) {
  return [...new Set((article.identifiers ?? []).filter(id => id.kind.toLowerCase() === kind || (kind === "doi" && id.kind.toLowerCase() === "biorxiv")).flatMap(id => normalize(kind, id.value) ?? []))];
}
/** Source-native identity first. Explicit arXiv versions remain exact; neither
 * title similarity nor a base-version match can enable an import. */
export function notificationLookup(record: DiscoveryRecord): NotificationLookup | null {
  const kind = ({ pubmed: "pmid", openalex: "openalex", arxiv: "arxiv", semantic_scholar: "s2", crossref: "doi" } as Record<string, string>)[record.source];
  const own = kind ? values(record.article, kind) : [];
  let chosen = own.length === 1 ? { kind, id: own[0] } : null;
  if (!chosen && record.source !== "arxiv") {
    const dois = values(record.article, "doi");
    if (dois.length === 1) chosen = { kind: "doi", id: dois[0] };
  }
  if (!chosen) return null;
  const { id, kind: selectedKind } = chosen;
  let request: DiscoveryRequest;
  if (selectedKind === "pmid") request = { sources: ["pubmed"], native: { source: "pubmed", parameters: { term: `"${id}"[PMID]` } }, page_size: 1 };
  else if (selectedKind === "openalex") request = { sources: ["openalex"], native: { source: "openalex", parameters: { filter: `openalex:${id.toUpperCase()}` } }, page_size: 1 };
  else if (selectedKind === "s2") request = { sources: ["semantic_scholar"], native: { source: "semantic_scholar", parameters: { endpoint: "lookup", paper_id: id } }, page_size: 1 };
  else if (selectedKind === "arxiv") request = { sources: ["arxiv"], native: { source: "arxiv", parameters: { id_list: id } }, page_size: 1 };
  else request = { sources: [record.source === "openalex" ? "openalex" : record.source === "pubmed" ? "pubmed" : record.source === "semantic_scholar" ? "semantic_scholar" : "crossref"], query: id, page_size: 1 };
  return { request, label: `${request.sources[0]} · exact ${selectedKind.toUpperCase()} ${id}`, accepts: article => values(article, selectedKind).includes(id) };
}
