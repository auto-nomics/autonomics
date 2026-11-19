import { openWikiEntry } from "@/features/wiki/wiki-state";
import { openReaderEditor } from "@/features/reader/reader-opener";
import { openBibRecordEditor } from "@/features/bibliography/bib-record-opener";

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
      const query = params.get("query");
      const attachment = params.get("attachment");
      // A bare bib:// link (or an explicit ?view=record) jumps to the record
      // entry in the library; locator params open the attachment reader.
      if (params.get("view") !== "reader" && !attachment && !pageText && !query) {
        openBibRecordEditor({ articleId: id });
        return true;
      }
      openReaderEditor({ articleId: id, title: title || id, attachmentKey: attachment, search: { page, query: query ?? undefined } });
    }
    return true;
  } catch { return false; }
}
