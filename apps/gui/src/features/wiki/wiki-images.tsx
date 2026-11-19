import { useEffect, useRef, useState, type RefObject } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { wikiAttachmentRef } from "@/lib/wiki-attachment-ref";
import { attachmentRawUrl } from "./wiki-api";

// Share in-flight desktop downloads, and release every object URL after its last view closes.
const resources = new Map<string, { users: number; promise: Promise<string> }>();
function acquire(entryId: string, attachmentId: string) {
  const key = `${entryId}/${attachmentId}`;
  let resource = resources.get(key);
  if (!resource) {
    const promise = import("@tauri-apps/api/core").then(async ({ invoke }) => {
      const data = await invoke<{ data_base64: string; content_type: string }>("wiki_image_raw", { entryId, attachmentId });
      const bytes = Uint8Array.from(atob(data.data_base64), c => c.charCodeAt(0));
      return URL.createObjectURL(new Blob([bytes], { type: data.content_type }));
    });
    resource = { users: 0, promise };
    resources.set(key, resource);
  }
  resource.users++;
  const current = resource;
  return { promise: current.promise, release: () => {
    if (--current.users === 0) {
      resources.delete(key);
      void current.promise.then(url => URL.revokeObjectURL(url), () => {});
    }
  } };
}

/** Hydrate attachment images in React views and CodeMirror preview widgets alike. */
export function loadWikiImages(root: HTMLElement, onLoad?: () => void) {
  let cancelled = false;
  const releases: (() => void)[] = [];
  const errors: HTMLElement[] = [];
  const fail = (img: HTMLImageElement) => {
    if (img.hidden) return;
    img.hidden = true;
    const message = document.createElement("span");
    message.className = "wiki-image-error";
    message.textContent = `图片不存在或无法访问：${img.alt}`;
    img.after(message); errors.push(message);
    onLoad?.();
  };
  const onError = (event: Event) => { if (event.target instanceof HTMLImageElement) fail(event.target); };
  root.addEventListener("error", onError, true);
  if (onLoad) root.addEventListener("load", onLoad, true);
  const images = root instanceof HTMLImageElement ? [root] : root.querySelectorAll<HTMLImageElement>("img");
  images.forEach(img => {
    img.hidden = false;
    const attachment = wikiAttachmentRef(img.getAttribute("src") ?? "");
    if (attachment) { img.dataset.wikiEntry = attachment.entryId; img.dataset.wikiAttachment = attachment.attachmentId; }
    if ("__TAURI_INTERNALS__" in window && img.dataset.wikiEntry && img.dataset.wikiAttachment) {
      img.removeAttribute("src");
      const asset = acquire(img.dataset.wikiEntry, img.dataset.wikiAttachment);
      releases.push(asset.release);
      void asset.promise.then(url => { if (!cancelled) img.src = url; }, () => { if (!cancelled) fail(img); });
    } else if (img.complete && img.getAttribute("src") && img.naturalWidth === 0) fail(img);
  });
  return () => {
    cancelled = true;
    root.removeEventListener("error", onError, true);
    if (onLoad) root.removeEventListener("load", onLoad, true);
    releases.forEach(release => release());
    errors.forEach(error => error.remove());
  };
}

export function useWikiImages(ref: RefObject<HTMLElement | null>, revision: string, preview = false) {
  const [opened, setOpened] = useState<{ src: string; alt: string } | null>(null);
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const release = loadWikiImages(root);
    const click = (event: MouseEvent) => {
      if (preview && event.target instanceof HTMLImageElement && event.target.src) {
        event.preventDefault();
        setOpened({ src: event.target.src, alt: event.target.alt });
      }
    };
    root.addEventListener("click", click);
    return () => { root.removeEventListener("click", click); release(); };
  }, [ref, revision, preview]);
  return <Dialog open={opened !== null} onOpenChange={open => { if (!open) setOpened(null); }}>
    <DialogContent className="max-w-[90vw]">
      <DialogHeader><DialogTitle>{opened?.alt || "图片预览"}</DialogTitle><DialogDescription>原始图片</DialogDescription></DialogHeader>
      {opened && <img src={opened.src} alt={opened.alt} className="max-h-[80vh] max-w-full object-contain" />}
    </DialogContent>
  </Dialog>;
}

export function WikiAttachmentImage({ entryId, attachmentId, filename }: { entryId: string; attachmentId: string; filename: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const preview = useWikiImages(ref, `${entryId}/${attachmentId}`, true);
  return <><div ref={ref}><img data-wiki-entry={entryId} data-wiki-attachment={attachmentId} src={"__TAURI_INTERNALS__" in window ? undefined : attachmentRawUrl(entryId, attachmentId)} alt={filename} className="h-20 w-full cursor-zoom-in rounded object-contain" loading="lazy" /></div>{preview}</>;
}
