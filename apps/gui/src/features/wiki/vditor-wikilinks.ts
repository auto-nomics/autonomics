import type Vditor from "vditor";
import { toast } from "sonner";
import { findEntryByTitle } from "./wiki-api";
import { openWikiEntry } from "./wiki-state";

const excluded = 'code, kbd, script, style, textarea, [data-type="code-block"], [data-type="math-block"], [data-type="inline-math"], [data-type="a"], .vditor-ir__marker, .vditor-ir__preview, .katex, [data-wiki-link-source]';

/** Display links as atomic chips; Lute always receives their original syntax. */
export function observeWikiEditorLinks(editor: Vditor) {
  const { element: root, lute } = editor.vditor;
  const entries = new Map<string, ReturnType<typeof findEntryByTitle>>();
  let disposed = false;
  let queued = false;
  const sourceHTML = (html: string) => {
    if (!html.includes("data-wiki-link-source")) return html;
    const template = document.createElement("template");
    template.innerHTML = html;
    template.content.querySelectorAll<HTMLElement>("[data-wiki-link-source]").forEach(link => {
      link.replaceWith(document.createTextNode(link.dataset.wikiLinkSource!));
    });
    return template.innerHTML;
  };
  // Input normalization, mode changes, clipboard and save use the same adapter.
  const irMarkdown = lute.VditorIRDOM2Md.bind(lute);
  const wysiwygMarkdown = lute.VditorDOM2Md.bind(lute);
  const irDOM = lute.SpinVditorIRDOM.bind(lute);
  const wysiwygDOM = lute.SpinVditorDOM.bind(lute);
  lute.VditorIRDOM2Md = html => irMarkdown(sourceHTML(html));
  lute.VditorDOM2Md = html => wysiwygMarkdown(sourceHTML(html));
  lute.SpinVditorIRDOM = html => irDOM(sourceHTML(html));
  lute.SpinVditorDOM = html => wysiwygDOM(sourceHTML(html));

  const resolve = (target: string) => {
    let entry = entries.get(target);
    if (!entry) {
      entry = findEntryByTitle(target).catch(() => null);
      entries.set(target, entry);
    }
    return entry;
  };
  const scan = () => {
    queued = false;
    if (disposed) return;
    const selection = window.getSelection();
    root.querySelectorAll<HTMLElement>('[contenteditable="true"], .vditor-preview .vditor-reset').forEach(body => {
      const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      const nodes: Text[] = [];
      while (walker.nextNode()) nodes.push(walker.currentNode as Text);
      for (const text of nodes) {
        if (!text.data.includes("[[") || text.parentElement?.closest(excluded)) continue;
        // Keep source under the caret stable while the user types or edits it.
        if (root.contains(document.activeElement) && (selection?.anchorNode === text || selection?.focusNode === text)) continue;
        const fragment = document.createDocumentFragment();
        let offset = 0;
        for (const match of text.data.matchAll(/\[\[([^\[\]\n]+)\]\]/g)) {
          const pipe = match[1].indexOf("|");
          const target = (pipe < 0 ? match[1] : match[1].slice(0, pipe)).trim();
          if (!target) continue;
          const display = pipe < 0 ? target : match[1].slice(pipe + 1).trim() || target;
          fragment.append(text.data.slice(offset, match.index));
          const link = document.createElement("span");
          link.className = "md-wikilink";
          link.contentEditable = "false";
          link.dataset.wikilink = target;
          link.dataset.wikiLinkSource = match[0];
          link.textContent = display;
          link.title = `${target} · Ctrl/Cmd+点击打开`;
          fragment.append(link);
          offset = match.index! + match[0].length;
          void resolve(target).then(entry => {
            if (disposed || !link.isConnected) return;
            link.classList.toggle("md-wikilink--missing", !entry);
            link.classList.toggle("md-wikilink--resolved", !!entry);
            link.title = entry ? `${entry.title}\n${entry.content.slice(0, 200)}\nCtrl/Cmd+点击打开` : `${target} · 词条尚不存在`;
          });
        }
        if (offset) { fragment.append(text.data.slice(offset)); text.replaceWith(fragment); }
      }
    });
  };
  const schedule = () => {
    if (disposed || queued) return;
    queued = true;
    queueMicrotask(scan);
  };
  const activate = (event: MouseEvent) => {
    if (!(event.target instanceof Element)) return;
    const link = event.target.closest<HTMLElement>("[data-wiki-link-source]");
    if (!link || !root.contains(link) || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.ctrlKey || event.metaKey) {
      void resolve(link.dataset.wikilink!).then(entry => {
        if (disposed) return;
        if (entry) openWikiEntry(entry.id, { title: entry.title, forceNew: true });
        else toast.info(`词条「${link.dataset.wikilink}」尚不存在`);
      });
      return;
    }
    const text = document.createTextNode(link.dataset.wikiLinkSource!);
    link.replaceWith(text);
    editor.focus();
    const range = document.createRange();
    range.setStart(text, text.length - 2);
    range.collapse(true);
    selectionTo(range);
  };
  const observer = new MutationObserver(schedule);
  observer.observe(root, { childList: true, subtree: true, characterData: true });
  document.addEventListener("selectionchange", schedule);
  root.addEventListener("mousedown", activate, true);
  schedule();
  return () => {
    disposed = true;
    observer.disconnect();
    document.removeEventListener("selectionchange", schedule);
    root.removeEventListener("mousedown", activate, true);
  };
}

function selectionTo(range: Range) {
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}
