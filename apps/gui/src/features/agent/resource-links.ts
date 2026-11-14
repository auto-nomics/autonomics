import { openWikiEntry } from "@/features/wiki/wiki-state";
import { openReaderEditor } from "@/features/reader/reader-opener";

/** Resource IDs are opaque and case-sensitive; do not normalize them as URL hostnames. */
export function openResourceLink(href: string, title: string): boolean {
  const match = /^(wiki|bib):\/\/([^/?#]+)(?:\?([^#]*))?$/.exec(href);
  if (!match) return false;
  try {
    const id = decodeURIComponent(match[2]);
    if (!id.trim() || /[\u0000-\u001f]/.test(id)) return false;
    const params = new URLSearchParams(match[3] ?? "");
    if (match[1] === "wiki") openWikiEntry(id, { title: title || id });
    else {
      const pageText = params.get("page");
      const page = pageText && /^[1-9]\d*$/.test(pageText) && Number.isSafeInteger(Number(pageText)) ? Number(pageText) : undefined;
      openReaderEditor({ articleId: id, title: title || id, attachmentKey: params.get("attachment"), search: { page, query: params.get("query") ?? undefined } });
    }
    return true;
  } catch { return false; }
}
