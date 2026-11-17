import { useEffect, useRef, type ReactNode } from "react";
import { EditorState, StateField } from "@codemirror/state";
import { EditorView, Decoration, WidgetType, keymap, hoverTooltip, drawSelection, type DecorationSet } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { syntaxTree } from "@codemirror/language";
import { autocompletion, acceptCompletion, type CompletionContext } from "@codemirror/autocomplete";
import { toast } from "sonner";
import { renderWikiMarkdown } from "./markdown";
import { findEntryByTitle, getEntry, listEntries, searchEntries, uploadAttachment } from "./wiki-api";
import { wikiImageMarkdown } from "@/lib/wiki-attachment-ref";

/** Map browser text hit-testing back to source, skipping Markdown delimiters. */
function sourceOffsetAtPoint(dom: HTMLElement, source: string, x: number, y: number): number {
  const doc = dom.ownerDocument as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  const caret = doc.caretPositionFromPoint?.(x, y);
  const range = caret ? null : doc.caretRangeFromPoint?.(x, y);
  const node = caret?.offsetNode ?? range?.startContainer;
  const offset = caret?.offset ?? range?.startOffset ?? 0;
  if (!node || !dom.contains(node)) return 0;
  const walker = doc.createTreeWalker(dom, NodeFilter.SHOW_TEXT);
  let position = 0;
  while (walker.nextNode()) {
    const textNode = walker.currentNode;
    const text = textNode.textContent ?? "";
    const exact = source.indexOf(text, position);
    if (exact >= 0) {
      if (textNode === node) return exact + Math.min(offset, text.length);
      position = exact + text.length;
    } else {
      // Renderers may normalize whitespace or decode entities. Align visible
      // characters monotonically rather than counting hidden syntax as text.
      for (let index = 0; index < text.length; index++) {
        if (textNode === node && index === offset) return position;
        const next = source.indexOf(text[index], position);
        if (next >= 0) position = next + 1;
      }
      if (textNode === node) return position;
    }
  }
  return Math.min(position, source.length);
}

class Preview extends WidgetType {
  constructor(readonly source: string, readonly entryId: string) { super(); }
  eq(other: Preview) { return other.source === this.source && other.entryId === this.entryId; }
  toDOM(view: EditorView) {
    const dom = document.createElement("div");
    dom.className = "md wiki-live-preview";
    let alive = true;
    renderWikiMarkdown(this.source, this.entryId).then(html => {
      if (!alive) return;
      dom.innerHTML = html;
      view.requestMeasure();
    }).catch(() => { dom.textContent = this.source; });
    dom.addEventListener("mousedown", event => {
      if (event.button !== 0) return;
      event.preventDefault();
      const pos = view.posAtDOM(dom) + sourceOffsetAtPoint(dom, this.source, event.clientX, event.clientY);
      view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
      view.focus();
    });
    (dom as HTMLElement & { dispose?: () => void }).dispose = () => { alive = false; };
    return dom;
  }
  destroy(dom: HTMLElement) { (dom as HTMLElement & { dispose?: () => void }).dispose?.(); }
  ignoreEvent() { return true; }
}

export function LiveEditor({ entryId, initialContent, onChange, onUploadsChange, header }: {
  entryId: string; initialContent: string; onChange: (content: string) => void;
  onUploadsChange?: (pending: boolean) => void; getContent?: () => string; header?: ReactNode;
}) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<EditorView | null>(null);
  const callbacks = useRef({ onChange, onUploadsChange });
  callbacks.current = { onChange, onUploadsChange };
  useEffect(() => {
    if (!host.current) return;
    let disposed = false;
    let pending = 0;
    const previews = StateField.define<DecorationSet>({
      create: () => Decoration.none,
      update(_previous, transaction) {
        const state = transaction.state;
        const ranges: ReturnType<ReturnType<typeof Decoration.replace>["range"]>[] = [];
        // Top-level syntax nodes preserve lists, tables and fenced code as units.
        const cursor = syntaxTree(state).cursor();
        if (cursor.firstChild()) do {
          const { from, to } = cursor;
          const selected = state.selection.ranges.some(range => range.from <= to && range.to >= from);
          if (!selected && to > from) ranges.push(Decoration.replace({
            widget: new Preview(state.doc.sliceString(from, to), entryId), block: true,
          }).range(from, to));
        } while (cursor.nextSibling());
        return Decoration.set(ranges, true);
      },
      provide: field => EditorView.decorations.from(field),
    });
    async function complete(context: CompletionContext) {
      const match = context.matchBefore(/\[\[([^\[\]\n|]*)/);
      if (!match) return null;
      const query = match.text.slice(2).trim();
      const entries = query
        ? await Promise.all((await searchEntries(query, { limit: 8 })).map(hit => getEntry(hit.entry_id)))
        : await listEntries({ limit: 8 });
      return { from: match.from + 2, options: entries.map(entry => ({
        label: entry.title, detail: entry.alias.join(" · "),
        apply(view: EditorView, _completion: unknown, from: number, to: number) {
          const end = view.state.doc.sliceString(to, to + 2) === "]]" ? to + 2 : to;
          view.dispatch({ changes: { from, to: end, insert: `${entry.title}]]` }, selection: { anchor: from + entry.title.length + 2 } });
        },
        async info() {
          const full = await getEntry(entry.id);
          const dom = document.createElement("div");
          dom.className = "md p-3 max-w-sm";
          const title = document.createElement("strong"); title.textContent = full.title;
          const text = document.createElement("p"); text.textContent = full.content.slice(0, 400);
          dom.append(title, text); return dom;
        },
      })) };
    }
    const view = new EditorView({ parent: host.current, state: EditorState.create({
      doc: initialContent,
      extensions: [markdown({ base: markdownLanguage }), history(), drawSelection(), previews, EditorView.lineWrapping,
        keymap.of([{ key: "Tab", run: acceptCompletion }, ...defaultKeymap, ...historyKeymap]),
        hoverTooltip(async (view, pos) => {
          const line = view.state.doc.lineAt(pos);
          for (const match of line.text.matchAll(/\[\[([^\]\n|]+)(?:\|[^\]\n]*)?\]\]/g)) {
            const from = line.from + match.index!;
            if (pos < from || pos > from + match[0].length) continue;
            const entry = await findEntryByTitle(match[1].trim());
            if (!entry) return null;
            return { pos: from, end: from + match[0].length, above: false, create() {
              const dom = document.createElement("div"); dom.className = "p-3 max-w-sm";
              const title = document.createElement("strong"); title.textContent = entry.title;
              const excerpt = document.createElement("p"); excerpt.textContent = entry.content.slice(0, 300);
              dom.append(title, excerpt); return { dom };
            } };
          }
          return null;
        }),
        autocompletion({ override: [complete], activateOnTyping: true }),
        EditorView.updateListener.of(update => {
          if (update.docChanged) callbacks.current.onChange(update.state.doc.toString());
        }),
        EditorView.domEventHandlers({
          paste(event, view) {
            const images = Array.from(event.clipboardData?.files ?? []).filter(file => file.type.startsWith("image/"));
            if (!images.length) return false;
            event.preventDefault();
            for (const file of images) {
              const marker = `<!--wiki-image-upload:${crypto.randomUUID()}-->`;
              view.dispatch(view.state.replaceSelection(marker));
              pending++; callbacks.current.onUploadsChange?.(true);
              void uploadAttachment(entryId, file).then(meta => {
                if (disposed) return;
                const from = view.state.doc.toString().indexOf(marker);
                if (from >= 0) view.dispatch({ changes: { from, to: from + marker.length, insert: wikiImageMarkdown(entryId, meta.attachment_id ?? meta.id ?? "", meta.filename) } });
              }).catch(error => {
                if (!disposed) {
                  const from = view.state.doc.toString().indexOf(marker);
                  if (from >= 0) view.dispatch({ changes: { from, to: from + marker.length, insert: "" } });
                  toast.error(String(error));
                }
              }).finally(() => { pending--; if (!disposed) callbacks.current.onUploadsChange?.(pending > 0); });
            }
            return true;
          },
        }),
        EditorView.theme({
          "&": { minHeight: "100%", backgroundColor: "transparent", color: "var(--foreground)" },
          ".cm-scroller": { overflow: "visible", fontFamily: "inherit" },
          ".cm-line": { padding: "0" },
          ".cm-content": { padding: "16px 24px", maxWidth: "816px", margin: "0 auto", width: "100%", lineHeight: "1.75", caretColor: "var(--foreground)" },
          ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--foreground)", borderLeftWidth: "2px" },
          "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: "var(--accent)" },
          "&.cm-focused": { outline: "none" },
          ".cm-tooltip": { backgroundColor: "var(--popover)", color: "var(--popover-foreground)", border: "1px solid var(--border)", borderRadius: "6px" },
          ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--accent)", color: "var(--accent-foreground)" },
        }),
      ],
    }) });
    editor.current = view;
    view.dispatch({});
    return () => { disposed = true; editor.current = null; callbacks.current.onUploadsChange?.(false); view.destroy(); };
    // The document is synchronized separately; entry changes recreate the editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entryId]);
  useEffect(() => {
    const view = editor.current;
    if (view && view.state.doc.toString() !== initialContent) view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: initialContent } });
  }, [initialContent]);
  return <div className="min-h-0 flex-1 overflow-auto"><div className="px-6 pt-4">{header}</div><div ref={host} /></div>;
}
