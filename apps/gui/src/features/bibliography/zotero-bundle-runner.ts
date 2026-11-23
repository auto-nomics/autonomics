import i18next from "i18next";
import { matchBundleFile, zoteroMetadataWarnings, type ZoteroBundle } from "./zotero-bundle-model";

type PreviewRow = { index: number; duplicate_id: string | null; article: { id: string } };
type Request = (method: string, path: string, body?: unknown) => Promise<any>;
type Upload = (id: string, file: File, options: { preferred: boolean; title?: string }) => Promise<any>;
export type BundleReport = { articles: number; collections: number; notes: number; attachments: number; skipped: number; errors: string[]; warnings: string[] };
export async function migrateZoteroBundle({ contents, plan, files, sourceLabel, request, upload, noteText, progress }: {
  contents: string; plan: ZoteroBundle; files: File[]; sourceLabel: string; request: Request; upload: Upload;
  noteText: (html: string) => string; progress: (message: string) => void;
}): Promise<BundleReport> {
  const report: BundleReport = { articles: 0, collections: 0, notes: 0, attachments: 0, skipped: 0, errors: [], warnings: [...plan.warnings] };
  const digest = async (value: string | ArrayBuffer) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", typeof value === "string" ? new TextEncoder().encode(value) : value))).map(byte => byte.toString(16).padStart(2, "0")).join("");
  const scope = (await digest(sourceLabel.trim())).slice(0, 16);
  progress(i18next.t("bibliography:zoteroRunner.importingMetadata"));
  const committed = await request("POST", "/bib/import/commit", { format: "zotero_json", contents, selected_indices: plan.records.map(record => record.index) });
  report.articles = committed.imported_ids.length;
  const preview = await request("POST", "/bib/import/preview", { format: "zotero_json", contents });
  report.warnings.push(...zoteroMetadataWarnings(preview.records));
  const ids = new Map<number, string>();
  for (const row of preview.records as PreviewRow[]) if (row.duplicate_id) ids.set(row.index, row.duplicate_id);
  if (ids.size !== plan.records.length) throw new Error(i18next.t("bibliography:zoteroRunner.mappingFailed"));
  const existingCollections = (await request("GET", "/bib/collections")).collections as Array<{id: string; description?: string; deleted?: boolean}>;
  const collectionIds = new Map<string, string>();
  const pending = [...plan.collections];
  while (pending.length) {
    const next = pending.findIndex(collection => !collection.parent || collectionIds.has(collection.parent));
    if (next < 0) { report.errors.push(i18next.t("bibliography:zoteroRunner.childCollectionFailed")); break; }
    const [collection] = pending.splice(next, 1);
    progress(i18next.t("bibliography:zoteroRunner.restoringCollection", { name: collection.name }));
    try {
      const marker = `Zotero import ${scope}:${await digest(collection.key)}`;
      const matches = existingCollections.filter(value => value.description === marker && !value.deleted);
      if (matches.length > 1) throw new Error(i18next.t("bibliography:zoteroRunner.duplicateCollectionMatch", { name: collection.name }));
      const stored = matches[0] || (await request("POST", "/bib/collections", { name: collection.name, parent_id: collection.parent ? collectionIds.get(collection.parent) : null, description: marker })).collection;
      collectionIds.set(collection.key, stored.id); if (!matches.length) report.collections++;
    } catch (error) { report.errors.push(`${collection.name}: ${String(error)}`); }
  }
  for (const record of plan.records) {
    const articleId = ids.get(record.index)!; const path = `/bib/articles/${encodeURIComponent(articleId)}`;
    progress(i18next.t("bibliography:zoteroRunner.restoringNotes", { current: record.index + 1, total: plan.records.length }));
    let detail: any;
    try { detail = await request("GET", path); } catch (error) { report.errors.push(`${record.key}: ${String(error)}`); continue; }
    const memberships = new Set([...record.collections, ...plan.collections.filter(collection => collection.items.some(id => record.aliases.includes(id))).map(collection => collection.key)]);
    for (const key of memberships) if (collectionIds.has(key)) {
      try { await request("POST", `/bib/collections/${encodeURIComponent(collectionIds.get(key)!)}/articles`, {article_id: articleId}); }
      catch (error) { report.errors.push(`Membership ${key}: ${String(error)}`); }
    }
    const notes: Array<{ id: string; content: string; tags?: string[] }> = detail.annotations || [];
    for (const note of record.notes) {
      const raw = typeof note.data.note === "string" ? note.data.note : "";
      const content = noteText(raw).trim();
      if (!content) { report.skipped++; continue; }
      try {
        const marker = `zotero-note:${scope}:${(await digest(note.key)).slice(0, 32)}`;
        const previous = notes.find(value => value.tags?.includes(marker));
        if (previous) {
          if (previous.content !== content) report.warnings.push(i18next.t("bibliography:zoteroRunner.noteDiffers", { key: note.key }));
          report.skipped++; continue;
        }
        // Recover safely if creation succeeded but its provenance-tag write was interrupted.
        let saved = notes.find(value => value.content === content);
        if (!saved) { saved = (await request("POST", `${path}/annotations`, { kind: "comment", content })).annotation; notes.push(saved!); report.notes++; }
        else report.skipped++;
        await request("PATCH", `/bib/annotations/${encodeURIComponent(saved!.id)}`, { tags: [...new Set([...(saved!.tags || []), marker])] });
        saved!.tags = [...new Set([...(saved!.tags || []), marker])];
      } catch (error) { report.errors.push(`Note ${note.key}: ${String(error)}`); }
    }
    const storedHashes = new Set<string>((detail.attachments || []).map((attachment: { storage_hash?: string }) => attachment.storage_hash).filter(Boolean));
    let hasPreferred = Boolean(detail.fulltext);
    for (const attachment of record.attachments) {
      const sourcePath = typeof attachment.data.path === "string" ? attachment.data.path : "";
      try {
        const file = matchBundleFile(sourcePath, files);
        if (!file) { report.warnings.push(i18next.t("bibliography:zoteroRunner.attachmentMissing", { name: sourcePath || attachment.key })); report.skipped++; continue; }
        if (file.size > 50 * 1024 * 1024) { report.warnings.push(i18next.t("bibliography:zoteroRunner.attachmentTooLarge", { name: file.name })); report.skipped++; continue; }
        const hash = await digest(await file.arrayBuffer());
        if (storedHashes.has(hash)) { report.skipped++; continue; }
        await upload(articleId, file, { preferred: !hasPreferred, title: typeof attachment.data.title === "string" ? attachment.data.title : undefined });
        storedHashes.add(hash); hasPreferred = true; report.attachments++;
      } catch (error) { report.errors.push(`Attachment ${sourcePath || attachment.key}: ${String(error)}`); }
    }
  }
  report.warnings = [...new Set(report.warnings)];
  return report;
}
