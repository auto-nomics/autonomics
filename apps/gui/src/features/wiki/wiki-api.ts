/** Wiki REST client over gatewayRequest — mirrors `/api/v1/wiki`. */
import { gatewayRequest } from "@/gateway";

export interface WikiEntry {
  id: string;
  title: string;
  alias: string[];
  tags: string[];
  content: string;
  ctime: number;
  mtime: number;
}

/** Browse/list payload: entry summary without `content`. */
export interface EntrySummary {
  id: string;
  title: string;
  alias: string[];
  tags: string[];
  ctime: number;
  mtime: number;
}

export interface SearchHit {
  entry_id: string;
  score: number;
}

export interface RecycleItem {
  entry: WikiEntry;
  deleted_at: number;
}

export interface VersionSummary {
  version: number;
  created_at: number;
}

export interface AttachmentMeta {
  attachment_id?: string;
  id?: string;
  entry_id?: string;
  filename: string;
  mime: string;
  size: number;
  sha256: string;
  created_at?: number;
}

/** gatewayRequest returns untyped payloads; narrow here. */
async function get<T>(path: string): Promise<T> {
  return (await gatewayRequest("GET", path)) as T;
}

function tagsQuery(tags: string[]): string {
  return tags.length ? `&tags=${encodeURIComponent(tags.join(","))}` : "";
}

export async function listEntries(options: {
  tags?: string[];
  since?: number;
  until?: number;
  limit?: number;
  offset?: number;
} = {}): Promise<EntrySummary[]> {
  const params = new URLSearchParams();
  if (options.limit !== undefined) params.set("limit", String(options.limit));
  if (options.offset !== undefined) params.set("offset", String(options.offset));
  if (options.since !== undefined) params.set("since", String(options.since));
  if (options.until !== undefined) params.set("until", String(options.until));
  const data = await get<{ entries: EntrySummary[] }>(
    `/wiki/entries?${params.toString()}${tagsQuery(options.tags ?? [])}`,
  );
  return data.entries;
}

export async function findEntryByTitle(title: string): Promise<WikiEntry | null> {
  const data = await get<{ entries: WikiEntry[] }>(
    `/wiki/entries?title=${encodeURIComponent(title)}`,
  );
  return data.entries[0] ?? null;
}

export async function getEntry(id: string): Promise<WikiEntry> {
  return get<WikiEntry>(`/wiki/entries/${encodeURIComponent(id)}`);
}

export async function createEntry(body: {
  title: string;
  content: string;
  alias?: string[];
  tags?: string[];
}): Promise<{ id: string; title: string }> {
  return gatewayRequest("POST", "/wiki/entries", body);
}

export async function updateEntryMeta(
  id: string,
  body: { new_title?: string; alias?: string[]; tags?: string[] },
): Promise<WikiEntry> {
  return gatewayRequest("PUT", `/wiki/entries/${encodeURIComponent(id)}`, body);
}

export async function putEntryContent(id: string, content: string): Promise<void> {
  await gatewayRequest("PUT", `/wiki/entries/${encodeURIComponent(id)}/content`, { content });
}

export async function deleteEntry(id: string): Promise<void> {
  await gatewayRequest("DELETE", `/wiki/entries/${encodeURIComponent(id)}`);
}

export async function restoreEntry(id: string): Promise<void> {
  await gatewayRequest("POST", `/wiki/entries/${encodeURIComponent(id)}/restore`);
}

export async function purgeEntry(id: string): Promise<void> {
  await gatewayRequest("DELETE", `/wiki/entries/${encodeURIComponent(id)}/purge`);
}

export async function listRecycle(): Promise<RecycleItem[]> {
  const data = await get<{ items: RecycleItem[] }>("/wiki/recycle");
  return data.items;
}

export async function listVersions(id: string): Promise<VersionSummary[]> {
  const data = await get<{ versions: VersionSummary[] }>(
    `/wiki/entries/${encodeURIComponent(id)}/versions`,
  );
  return data.versions;
}

export async function restoreVersion(id: string, version: number): Promise<void> {
  await gatewayRequest(
    "POST",
    `/wiki/entries/${encodeURIComponent(id)}/versions/${version}/restore`,
  );
}

export async function searchEntries(
  query: string,
  options: { limit?: number; tags?: string[] } = {},
): Promise<SearchHit[]> {
  const params = new URLSearchParams({ q: query });
  if (options.limit !== undefined) params.set("limit", String(options.limit));
  const data = await get<{ results: SearchHit[] }>(
    `/wiki/search?${params.toString()}${tagsQuery(options.tags ?? [])}`,
  );
  return data.results;
}

export interface LinkGraph {
  entry: { id: string; title: string };
  out_links: string[];
  in_links: string[];
}

export async function getLinks(id: string): Promise<LinkGraph> {
  return get<LinkGraph>(`/wiki/entries/${encodeURIComponent(id)}/links`);
}

export async function getMentions(id: string): Promise<string[]> {
  const data = await get<{ unlinked_mentions: string[] }>(
    `/wiki/entries/${encodeURIComponent(id)}/mentions`,
  );
  return data.unlinked_mentions;
}

export async function listAttachments(id: string): Promise<AttachmentMeta[]> {
  const data = await get<{ attachments: AttachmentMeta[] }>(
    `/wiki/entries/${encodeURIComponent(id)}/attachments`,
  );
  return data.attachments;
}

/**
 * Multipart upload — gatewayRequest is JSON-only, so this mirrors
 * `uploadBibFulltext`'s browser path. Tauri invoke lacks a generic
 * multipart command; the web app is the supported surface for now.
 */
export async function uploadAttachment(id: string, file: File): Promise<AttachmentMeta> {
  const form = new FormData();
  form.append("file", file, file.name);
  const response = await fetch(
    `/api/v1/wiki/entries/${encodeURIComponent(id)}/attachments`,
    { method: "POST", credentials: "same-origin", body: form },
  );
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new Error(payload?.error ?? `Upload ${response.status}`);
  }
  return payload as AttachmentMeta;
}

export function attachmentRawUrl(entryId: string, attachmentId: string): string {
  return `/api/v1/wiki/entries/${encodeURIComponent(entryId)}/attachments/${encodeURIComponent(attachmentId)}/raw`;
}

export async function detachAttachment(entryId: string, attachmentId: string): Promise<void> {
  await gatewayRequest(
    "DELETE",
    `/wiki/entries/${encodeURIComponent(entryId)}/attachments/${encodeURIComponent(attachmentId)}`,
  );
}

export function formatTime(unix?: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
}
