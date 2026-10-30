/** Wiki REST client — mirrors `/api/v1/wiki` (see api-server/src/wiki.rs). */

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

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    const message = data && typeof data.error === "string" ? data.error : response.statusText;
    throw new Error(message);
  }
  return data as T;
}

function tagsQuery(tags: string[]): string {
  return tags.length ? `&tags=${encodeURIComponent(tags.join(","))}` : "";
}

/** Browse entries newest-first (no text query); optionally tag/mtime filtered. */
export async function listEntries(options: {
  tags?: string[];
  since?: number;
  until?: number;
  limit?: number;
  offset?: number;
}): Promise<EntrySummary[]> {
  const params = new URLSearchParams();
  if (options.limit !== undefined) params.set("limit", String(options.limit));
  if (options.offset !== undefined) params.set("offset", String(options.offset));
  if (options.since !== undefined) params.set("since", String(options.since));
  if (options.until !== undefined) params.set("until", String(options.until));
  const suffix = tagsQuery(options.tags ?? []);
  const data = await api<{ entries: EntrySummary[] }>(
    `/api/v1/wiki/entries?${params.toString()}${suffix}`,
  );
  return data.entries;
}

/** Exact-title lookup. */
export async function findEntryByTitle(title: string): Promise<WikiEntry | null> {
  const data = await api<{ entries: WikiEntry[] }>(
    `/api/v1/wiki/entries?title=${encodeURIComponent(title)}`,
  );
  return data.entries[0] ?? null;
}

export async function getEntry(id: string): Promise<WikiEntry> {
  return api<WikiEntry>(`/api/v1/wiki/entries/${encodeURIComponent(id)}`);
}

export async function createEntry(body: {
  title: string;
  content: string;
  alias?: string[];
  tags?: string[];
}): Promise<{ id: string; title: string }> {
  return api("/api/v1/wiki/entries", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** Metadata-only update (rename / alias / tags); content goes through putContent. */
export async function updateEntryMeta(
  id: string,
  body: { new_title?: string; alias?: string[]; tags?: string[] },
): Promise<WikiEntry> {
  return api(`/api/v1/wiki/entries/${encodeURIComponent(id)}`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** Whole-body content replacement (indexes/links/version snapshot rebuilt server-side). */
export async function putEntryContent(id: string, content: string): Promise<void> {
  await api(`/api/v1/wiki/entries/${encodeURIComponent(id)}/content`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ content }),
  });
}

export async function deleteEntry(id: string): Promise<void> {
  await api(`/api/v1/wiki/entries/${encodeURIComponent(id)}`, { method: "DELETE" });
}

export async function restoreEntry(id: string): Promise<void> {
  await api(`/api/v1/wiki/entries/${encodeURIComponent(id)}/restore`, { method: "POST" });
}

export async function purgeEntry(id: string): Promise<void> {
  await api(`/api/v1/wiki/entries/${encodeURIComponent(id)}/purge`, { method: "DELETE" });
}

export async function listRecycle(): Promise<RecycleItem[]> {
  const data = await api<{ items: RecycleItem[] }>("/api/v1/wiki/recycle");
  return data.items;
}

export async function listVersions(id: string): Promise<VersionSummary[]> {
  const data = await api<{ versions: VersionSummary[] }>(
    `/api/v1/wiki/entries/${encodeURIComponent(id)}/versions`,
  );
  return data.versions;
}

export async function getVersion(id: string, version: number): Promise<WikiEntry> {
  const data = await api<{ entry: WikiEntry }>(
    `/api/v1/wiki/entries/${encodeURIComponent(id)}/versions/${version}`,
  );
  return data.entry;
}

export async function restoreVersion(id: string, version: number): Promise<void> {
  await api(`/api/v1/wiki/entries/${encodeURIComponent(id)}/versions/${version}/restore`, {
    method: "POST",
  });
}

export async function searchEntries(
  query: string,
  options: { limit?: number; tags?: string[] } = {},
): Promise<SearchHit[]> {
  const params = new URLSearchParams({ q: query });
  if (options.limit !== undefined) params.set("limit", String(options.limit));
  const data = await api<{ results: SearchHit[] }>(
    `/api/v1/wiki/search?${params.toString()}${tagsQuery(options.tags ?? [])}`,
  );
  return data.results;
}

export interface LinkGraph {
  entry: { id: string; title: string };
  out_links: string[];
  in_links: string[];
}

export async function getLinks(id: string): Promise<LinkGraph> {
  return api<LinkGraph>(`/api/v1/wiki/entries/${encodeURIComponent(id)}/links`);
}

export async function getMentions(id: string): Promise<string[]> {
  const data = await api<{ unlinked_mentions: string[] }>(
    `/api/v1/wiki/entries/${encodeURIComponent(id)}/mentions`,
  );
  return data.unlinked_mentions;
}

export async function listAttachments(id: string): Promise<AttachmentMeta[]> {
  const data = await api<{ attachments: AttachmentMeta[] }>(
    `/api/v1/wiki/entries/${encodeURIComponent(id)}/attachments`,
  );
  return data.attachments;
}

export async function uploadAttachment(id: string, file: File): Promise<AttachmentMeta> {
  const form = new FormData();
  form.append("file", file);
  return api(`/api/v1/wiki/entries/${encodeURIComponent(id)}/attachments`, {
    method: "POST",
    body: form,
  });
}

export function attachmentRawUrl(entryId: string, attachmentId: string): string {
  return `/api/v1/wiki/entries/${encodeURIComponent(entryId)}/attachments/${encodeURIComponent(attachmentId)}/raw`;
}

export async function detachAttachment(entryId: string, attachmentId: string): Promise<void> {
  await api(
    `/api/v1/wiki/entries/${encodeURIComponent(entryId)}/attachments/${encodeURIComponent(attachmentId)}`,
    { method: "DELETE" },
  );
}

/** `unix` seconds → short local string. */
export function formatTime(unix?: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
}
