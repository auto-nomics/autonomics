import { wikiAttachmentRef, wikiAttachmentUrl } from "@/lib/wiki-attachment-ref";
import { loadWikiImages } from "./wiki-images";
import type Vditor from "vditor";

/** Keep display URLs out of Vditor's serialized Markdown. */
function wikiEditorHTML(html: string, sources: Map<string, string>): string {
  const clone = document.createElement("template");
  clone.innerHTML = html;
  clone.content.querySelectorAll<HTMLImageElement>("img").forEach(img => {
    const source = img.dataset.wikiSource ?? sources.get(img.getAttribute("src") ?? "");
    if (!source) return;
    img.setAttribute("src", source);
    img.removeAttribute("hidden");
  });
  clone.content.querySelectorAll(".wiki-image-error").forEach(error => error.remove());
  return clone.innerHTML;
}

export function observeWikiEditorImages(root: HTMLElement, entryId: string, lute: Vditor["vditor"]["lute"]) {
  const images = new Map<HTMLImageElement, { source: string; release: () => void }>();
  const sources = new Map<string, string>();
  // Vditor also serializes internally when switching modes, copying and
  // rendering previews. Normalize at that boundary, not just on save.
  const irMarkdown = lute.VditorIRDOM2Md.bind(lute);
  const wysiwygMarkdown = lute.VditorDOM2Md.bind(lute);
  lute.VditorIRDOM2Md = html => irMarkdown(wikiEditorHTML(html, sources));
  lute.VditorDOM2Md = html => wysiwygMarkdown(wikiEditorHTML(html, sources));
  let disposed = false;
  const scan = () => {
    if (disposed) return;
    for (const [img, resource] of images) if (!root.contains(img)) {
      resource.release();
      images.delete(img);
    }
    root.querySelectorAll<HTMLImageElement>("img").forEach(img => {
      const raw = img.getAttribute("src") ?? "";
      const previous = images.get(img);
      // A desktop blob URL is a temporary display value, including when
      // Vditor clones an image while changing editing modes.
      const source = sources.get(raw) ?? (raw.startsWith("blob:") || !raw ? img.dataset.wikiSource ?? raw : raw);
      const ref = wikiAttachmentRef(source, entryId);
      if (previous && (source === previous.source || source === wikiAttachmentUrl(img.dataset.wikiEntry!, img.dataset.wikiAttachment!))) {
        if (raw) sources.set(raw, previous.source);
        return;
      }
      previous?.release();
      images.delete(img);
      if (!ref) { delete img.dataset.wikiSource; return; }
      img.dataset.wikiSource = source;
      img.dataset.wikiEntry = ref.entryId;
      img.dataset.wikiAttachment = ref.attachmentId;
      sources.set(wikiAttachmentUrl(ref.entryId, ref.attachmentId), source);
      if ("__TAURI_INTERNALS__" in window) img.removeAttribute("src");
      else img.src = wikiAttachmentUrl(ref.entryId, ref.attachmentId);
      // loadWikiImages listens on this image itself as well as containers.
      images.set(img, { source, release: loadWikiImages(img) });
    });
  };
  const observer = new MutationObserver(scan);
  observer.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ["src"] });
  scan();
  return () => {
    disposed = true;
    observer.disconnect();
    images.forEach(resource => resource.release());
    images.clear();
  };
}
