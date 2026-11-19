import type Vditor from "vditor";

/** One document scroller contains properties and the active editor together. */
export function attachWikiDocumentLayout(editor: Vditor) {
  const view = editor.vditor;
  const content = view.element.querySelector<HTMLElement>(".vditor-content")!;
  const document = window.document.createElement("div");
  document.className = "wiki-vditor-document";
  const metadata = window.document.createElement("div");
  metadata.className = "wiki-editor-metadata px-6 pt-4";
  metadata.setAttribute("aria-label", "文档元数据");
  const body = window.document.createElement("div");
  body.className = "wiki-vditor-body";
  const modes = [view.wysiwyg!.element.parentElement!, view.sv!.element, view.ir!.element.parentElement!, view.preview!.element];
  content.insertBefore(document, modes[0]);
  document.append(metadata, body);
  body.append(...modes);

  const layout = () => {
    if (editor.getCurrentMode() === "sv") {
      const source = view.sv!.element;
      source.style.height = "0px";
      source.style.height = `${Math.max(260, source.scrollHeight)}px`;
    }
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
