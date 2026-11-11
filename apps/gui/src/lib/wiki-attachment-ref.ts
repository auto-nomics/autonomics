/** Portable wiki image references. Legacy references resolve within their document. */
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function wikiAttachmentRef(src: string, currentEntryId?: string): { entryId: string; attachmentId: string } | null {
  const raw = /^\/api\/v1\/wiki\/entries\/([^/]+)\/attachments\/([^/]+)\/raw$/.exec(src);
  if (raw) return uuid.test(raw[1]) && uuid.test(raw[2]) ? { entryId: raw[1].toLowerCase(), attachmentId: raw[2].toLowerCase() } : null;
  const match = /^wiki-attach:\/\/([^/]+)\/([^/?#]+)$/.exec(src);
  if (!match || !uuid.test(match[1])) return null;
  if (uuid.test(match[2])) return { entryId: match[1].toLowerCase(), attachmentId: match[2].toLowerCase() };
  return currentEntryId && uuid.test(currentEntryId) ? { entryId: currentEntryId.toLowerCase(), attachmentId: match[1].toLowerCase() } : null;
}
export function wikiAttachmentUrl(entryId: string, attachmentId: string): string {
  return `/api/v1/wiki/entries/${encodeURIComponent(entryId)}/attachments/${encodeURIComponent(attachmentId)}/raw`;
}
export function wikiImageMarkdown(entryId: string, attachmentId: string, filename: string): string {
  if (!uuid.test(entryId) || !uuid.test(attachmentId)) throw new Error("图片附件 ID 无效");
  const alt = filename.replace(/[\r\n]/g, " ").replace(/[\\[\]<>`*_$]/g, "\\$&");
  return `![${alt}](wiki-attach://${entryId}/${attachmentId})`;
}
