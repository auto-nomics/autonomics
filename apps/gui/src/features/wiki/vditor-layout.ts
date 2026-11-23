import type Vditor from "vditor";
import i18n from "@/i18n";

const svSplitKey = "autonomics.wiki.svSplit";

/**
 * Drag handle between the source and preview panes of split ("sv") mode.
 *
 * The two panes are siblings in `.wiki-vditor-body` (a flex row): the
 * source textarea (`textarea.vditor-sv`, `flex-basis: 50%` from app CSS)
 * and `.vditor-preview` (`flex: 1`). The handle sits between them and only
 * rewrites the textarea's inline `flex-basis` percentage — the preview
 * fills the rest. The ratio persists in localStorage. Outside sv mode the
 * handle is hidden along with the source pane.
 */
function attachSvSplit(editor: Vditor, body: HTMLElement) {
  const view = editor.vditor;
  const textarea = view.sv!.element;
  const preview = view.preview!.element;
  if (!textarea || !preview || body.dataset.svSplit) return;
  body.dataset.svSplit = "1";

  const handle = window.document.createElement("div");
  handle.className = "wiki-sv-split-handle";
  handle.setAttribute("role", "separator");
  handle.setAttribute("aria-orientation", "vertical");
  handle.setAttribute("aria-label", i18n.t("wiki:vditor.layoutDragAria"));
  handle.title = i18n.t("wiki:vditor.layoutDragTitle");
  body.insertBefore(handle, preview);

  const apply = (percent: number) => {
    textarea.style.flexBasis = `${percent}%`;
  };
  const saved = Number.parseFloat(window.localStorage.getItem(svSplitKey) ?? "");
  if (Number.isFinite(saved) && saved >= 20 && saved <= 80) apply(saved);

  handle.addEventListener("pointerdown", (down) => {
    down.preventDefault();
    handle.setPointerCapture(down.pointerId);
    const move = (event: PointerEvent) => {
      const rect = body.getBoundingClientRect();
      const percent = Math.min(80, Math.max(20, ((event.clientX - rect.left) / rect.width) * 100));
      apply(percent);
      // Keep the auto-height pass in sync while dragging.
      textarea.style.height = "0px";
      textarea.style.height = `${Math.max(260, textarea.scrollHeight)}px`;
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      try {
        window.localStorage.setItem(svSplitKey, textarea.style.flexBasis);
      } catch { /* split preference is a convenience, not state */ }
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  });

  const sync = () => {
    handle.style.display = editor.getCurrentMode() === "sv" ? "" : "none";
  };
  sync();
  return sync;
}

/** One document scroller contains properties and the active editor together. */
export function attachWikiDocumentLayout(editor: Vditor) {
  const view = editor.vditor;
  const content = view.element.querySelector<HTMLElement>(".vditor-content")!;
  const document = window.document.createElement("div");
  document.className = "wiki-vditor-document";
  const metadata = window.document.createElement("div");
  metadata.className = "wiki-editor-metadata px-6 pt-4";
  metadata.setAttribute("aria-label", i18n.t("wiki:vditor.metadataAria"));
  const body = window.document.createElement("div");
  body.className = "wiki-vditor-body";
  const modes = [view.wysiwyg!.element.parentElement!, view.sv!.element, view.ir!.element.parentElement!, view.preview!.element];
  content.insertBefore(document, modes[0]);
  document.append(metadata, body);
  body.append(...modes);

  let syncSplit: (() => void) | undefined;
  const layout = () => {
    if (!syncSplit) syncSplit = attachSvSplit(editor, body);
    if (editor.getCurrentMode() === "sv") {
      const source = view.sv!.element;
      source.style.height = "0px";
      source.style.height = `${Math.max(260, source.scrollHeight)}px`;
    }
    syncSplit?.();
    content.style.setProperty("--wiki-editor-hint-offset", `${metadata.offsetHeight - document.scrollTop}px`);
  };
  const scroll = () => {
    view.hint.element.style.display = "none";
    layout();
  };
  document.addEventListener("scroll", scroll, { passive: true });
  const resize = new ResizeObserver(layout);
  resize.observe(document);
  resize.observe(metadata);
  return { metadata, layout, release() { resize.disconnect(); document.removeEventListener("scroll", scroll); } };
}
