import { useEffect, useRef, useState, type ReactNode } from "react";
import Vditor from "vditor";
import { createPortal } from "react-dom";
import "vditor/dist/index.css";
import { toast } from "sonner";
import { escapeHtml } from "@/lib/markdown";
import { wikiImageMarkdown } from "@/lib/wiki-attachment-ref";
import { getResolvedTheme, subscribeTheme } from "@/lib/theme";
import { getEntry, listEntries, searchEntries, uploadAttachment } from "./wiki-api";
import { prepareVditor, setVditorCodeTheme } from "./vditor-runtime";
import { observeWikiEditorImages } from "./vditor-images";
import { attachWikiDocumentLayout } from "./vditor-layout";
import { observeWikiEditorLinks } from "./vditor-wikilinks";
import { wikiMarkdownForVditor } from "./vditor-markdown";

type ImageUpload = { token: string; file: File; error?: string };
type EditingMode = ReturnType<Vditor["getCurrentMode"]>;
const modePreferenceKey = "autonomics.wiki.editingMode";
const editingModes = [
  { value: "wysiwyg", label: "所见即所得", description: "直接编辑排版后的正文 · Alt+Ctrl+7" },
  { value: "ir", label: "实时渲染", description: "编辑 Markdown，实时显示排版 · Alt+Ctrl+8" },
  { value: "sv", label: "双列编辑", description: "左侧 Markdown 源码，右侧实时预览 · Alt+Ctrl+9" },
] as const;

function preferredEditingMode(): EditingMode {
  try {
    const saved = localStorage.getItem(modePreferenceKey);
    if (saved === "wysiwyg" || saved === "ir" || saved === "sv") return saved;
  } catch { /* Editing also works when browser storage is unavailable. */ }
  return "ir";
}

const marker = (token: string) => `<!--wiki-image-upload:${token}-->`;

export function VditorEditor({ entryId, initialContent, onChange, onUploadsChange, getContent, header }: {
  entryId: string;
  initialContent: string;
  onChange: (content: string) => void;
  onUploadsChange?: (pending: boolean) => void;
  getContent: () => string;
  header?: ReactNode;
}) {
  const host = useRef<HTMLDivElement>(null);
  const callbacks = useRef({ onChange, onUploadsChange, getContent });
  callbacks.current = { onChange, onUploadsChange, getContent };
  const control = useRef<{ sync: (content: string) => void; switchMode: (mode: EditingMode) => void } | null>(null);
  const [ready, setReady] = useState(false);
  const [editingMode, setEditingMode] = useState(preferredEditingMode);
  const [toolbarHost, setToolbarHost] = useState<HTMLElement | null>(null);
  const [metadataHost, setMetadataHost] = useState<HTMLElement | null>(null);
  const [uploads, setUploads] = useState<ImageUpload[]>([]);
  const actions = useRef({ retry: (_token: string) => {}, remove: (_token: string) => {} });

  useEffect(() => {
    const root = host.current;
    if (!root) return;
    let disposed = false;
    let initialized = false;
    let releaseImages: (() => void) | undefined;
    let releaseLinks: (() => void) | undefined;
    let documentLayout: ReturnType<typeof attachWikiDocumentLayout> | undefined;
    const jobs = new Map<string, ImageUpload>();
    let lastPublished = callbacks.current.getContent();
    let lastNormalized = "";
    let currentMode = preferredEditingMode();
    prepareVditor();
    setVditorCodeTheme(getResolvedTheme() === "dark");

    const activeElement = () => instance.vditor[instance.getCurrentMode()]!.element;
    const read = () => instance.getValue();
    const refreshMode = () => {
      const next = instance.getCurrentMode();
      if (next === currentMode) return;
      currentMode = next;
      lastNormalized = read();
      setEditingMode(next);
      try { localStorage.setItem(modePreferenceKey, next); } catch { /* Session only. */ }
    };
    const publish = () => {
      if (disposed || !initialized) return;
      documentLayout?.layout();
      const value = read();
      if (value === lastNormalized) return;
      lastNormalized = value;
      lastPublished = value;
      callbacks.current.onChange(value);
    };
    const notify = () => {
      setUploads([...jobs.values()]);
      callbacks.current.onUploadsChange?.(jobs.size > 0);
    };
    const replaceMarker = (token: string, markdown: string) => {
      const element = activeElement();
      if (element instanceof HTMLTextAreaElement) {
        const from = element.value.indexOf(marker(token));
        if (from < 0) return;
        const start = element.selectionStart, end = element.selectionEnd;
        element.value = element.value.replace(marker(token), markdown);
        const map = (pos: number) => pos <= from ? pos : Math.max(from, pos + markdown.length - marker(token).length);
        element.setSelectionRange(map(start), map(end));
        instance.renderPreview();
      } else {
        // Replace only the upload's block, retaining the selection and edits
        // elsewhere in the document while network requests are in flight.
        const block = Array.from(element.querySelectorAll<HTMLElement>('[data-type="html-block"]'))
          .find(block => block.textContent?.includes(marker(token)));
        if (!block) return;
        block.outerHTML = instance.getCurrentMode() === "ir"
          ? instance.vditor.lute.Md2VditorIRDOM(markdown)
          : instance.vditor.lute.Md2VditorDOM(markdown);
      }
      publish();
    };
    const upload = async (job: ImageUpload) => {
      job.error = undefined;
      notify();
      try {
        const meta = await uploadAttachment(entryId, job.file);
        if (disposed || !jobs.has(job.token)) return;
        replaceMarker(job.token, wikiImageMarkdown(entryId, meta.attachment_id ?? meta.id ?? "", meta.filename));
        jobs.delete(job.token);
      } catch (error) {
        if (disposed || !jobs.has(job.token)) return;
        job.error = error instanceof Error ? error.message : String(error);
        toast.error(`图片上传失败：${job.error}`);
      }
      if (!disposed) notify();
    };
    const uploadImages = (files: File[]) => {
      if (disposed || !initialized) return;
      const images = files.filter(file => file?.type.startsWith("image/"));
      if (!images.length) return;
      const added = images.map(file => ({ token: crypto.randomUUID(), file }));
      for (const job of added) jobs.set(job.token, job);
      notify();
      instance.focus();
      instance.insertMD(`\n\n${added.map(job => marker(job.token)).join("\n\n")}\n\n`);
      if (!(activeElement() instanceof HTMLTextAreaElement)) {
        const blocks = Array.from(activeElement().querySelectorAll<HTMLElement>('[data-type="html-block"]'));
        const last = blocks.find(block => block.textContent?.includes(marker(added[added.length - 1].token)));
        if (last) {
          const paragraph = document.createElement("p");
          paragraph.dataset.block = "0";
          paragraph.append(document.createElement("br"));
          last.after(paragraph);
          const range = document.createRange();
          range.setStart(paragraph, 0);
          range.collapse(true);
          window.getSelection()?.removeAllRanges();
          window.getSelection()?.addRange(range);
        }
      }
      publish();
      for (const job of added) void upload(job);
    };
    actions.current = {
      retry(token) { const job = jobs.get(token); if (job?.error) void upload(job); },
      remove(token) { jobs.delete(token); replaceMarker(token, ""); notify(); },
    };
    const instance = new Vditor(root, {
      cdn: "/vditor",
      cache: { enable: false }, // The shared WikiDocument owns drafts, including split panes.
      value: wikiMarkdownForVditor(lastPublished),
      mode: currentMode,
      lang: "zh_CN",
      theme: getResolvedTheme() === "dark" ? "dark" : "classic",
      height: "100%",
      minHeight: 260,
      placeholder: "开始编写，输入 [[ 链接其他词条…",
      undoDelay: 100,
      toolbar: ["headings", "bold", "italic", "strike", "|", "list", "ordered-list", "check", "quote", "|", "code", "inline-code", "link", "upload", "table", "|", "undo", "redo", "|", "outline", "edit-mode"],
      preview: {
        delay: 200,
        maxWidth: 816,
        theme: { current: "", path: "" },
        hljs: { style: "github", lineNumber: false },
        markdown: { autoSpace: false, fixTermTypo: false, sanitize: true },
        actions: [],
      },
      hint: {
        parse: false,
        extend: [{ key: "[[", async hint(query) {
          if (query.includes("]]")) return [];
          try {
            const entries = query.trim()
              ? await Promise.all((await searchEntries(query.trim(), { limit: 8 })).map(hit => getEntry(hit.entry_id)))
              : await listEntries({ limit: 8 });
            if (disposed) return [];
            return entries.map(entry => ({ html: `<span>${escapeHtml(entry.title)}</span>`, value: `[[${escapeHtml(entry.title)}]]` }));
          } catch { return []; }
        } }],
      },
      upload: { accept: "image/*", multiple: true, max: 50 * 1024 * 1024, handler(files) { uploadImages(files); return null; } },
      input: publish,
      after() {
        if (disposed) { instance.destroy(); return; }
        initialized = true;
        documentLayout = attachWikiDocumentLayout(instance);
        setMetadataHost(documentLayout.metadata);
        const toolbar = instance.vditor.toolbar!;
        const nativeModeMenu = toolbar.elements!["edit-mode"];
        setToolbarHost(toolbar.element!);
        nativeModeMenu.setAttribute("data-wiki-mode-menu", "");
        releaseImages = observeWikiEditorImages(root, entryId, instance.vditor.lute);
        releaseLinks = observeWikiEditorLinks(instance);
        lastNormalized = read();
        root.querySelectorAll('[contenteditable="true"], textarea.vditor-sv').forEach(element => element.setAttribute("aria-label", "Wiki 正文"));
        setReady(true);
        control.current = {
          sync(value) {
            if (value === lastPublished) return;
            instance.setValue(wikiMarkdownForVditor(value));
            documentLayout?.layout();
            lastPublished = value;
            lastNormalized = read();
          },
          switchMode(next) {
            if (next === instance.getCurrentMode()) return;
            publish();
            // Reuse Vditor's mode commands to retain its selection handling,
            // undo stacks and document conversion.
            nativeModeMenu.querySelector<HTMLButtonElement>(`button[data-mode="${next}"]`)!.click();
            if (next === "sv") instance.setPreviewMode("both");
            refreshMode();
            documentLayout?.layout();
          },
        };
        control.current.sync(callbacks.current.getContent());
      },
    });
    const paste = (event: ClipboardEvent) => {
      if (!event.clipboardData || event.defaultPrevented) return;
      const data = event.clipboardData;
      const items = Array.from(data.items ?? []).filter(item => item.kind === "file" && item.type.startsWith("image/"))
        .map(item => item.getAsFile()).filter((file): file is File => file !== null);
      const images = items.length ? items : Array.from(data.files ?? []).filter(file => file.type.startsWith("image/"));
      if (!images.length) return;
      event.preventDefault();
      event.stopPropagation();
      uploadImages(images);
    };
    // Publish synchronously after Vditor handles input, before Ctrl+S or a
    // split-pane update can observe an older draft during its undo debounce.
    const input = (event: Event) => { if (!(event instanceof InputEvent) || !event.isComposing) publish(); };
    const afterCommand = (event: Event) => {
      queueMicrotask(() => {
        if (disposed) return;
        refreshMode();
        documentLayout?.layout();
      });
      if (event instanceof KeyboardEvent) {
        if (!((event.ctrlKey || event.metaKey) && /^(z|y)$/i.test(event.key)) && event.key !== "Enter") return;
      } else if (event.target instanceof Element) {
        const button = event.target.closest("[data-type], [data-mode], [data-wiki-mode]");
        if (button?.hasAttribute("data-mode") || button?.hasAttribute("data-wiki-mode") || ["edit-mode", "outline"].includes(button?.getAttribute("data-type") ?? "")) return;
      }
      queueMicrotask(publish);
    };
    const keydown = (event: KeyboardEvent) => {
      const shortcuts: Record<string, EditingMode> = { Digit7: "wysiwyg", Digit8: "ir", Digit9: "sv" };
      const next = shortcuts[event.code];
      if (next && (event.ctrlKey || event.metaKey) && event.altKey && !event.shiftKey && !event.isComposing
        && event.target instanceof Element && event.target.closest('[contenteditable="true"], .vditor-sv')) {
        event.preventDefault();
        event.stopPropagation();
        control.current?.switchMode(next);
        return;
      }
      afterCommand(event);
    };
    root.addEventListener("input", input);
    root.addEventListener("compositionend", publish);
    root.addEventListener("paste", paste, true);
    root.addEventListener("click", afterCommand, true);
    root.addEventListener("keydown", keydown, true);
    const unsubscribeTheme = subscribeTheme(() => {
      if (initialized && !disposed) {
        const dark = getResolvedTheme() === "dark";
        instance.setTheme(dark ? "dark" : "classic");
        setVditorCodeTheme(dark);
      }
    });
    return () => {
      disposed = true;
      control.current = null;
      unsubscribeTheme();
      root.removeEventListener("input", input);
      root.removeEventListener("compositionend", publish);
      root.removeEventListener("paste", paste, true);
      root.removeEventListener("click", afterCommand, true);
      root.removeEventListener("keydown", keydown, true);
      releaseImages?.();
      releaseLinks?.();
      documentLayout?.release();
      const current = callbacks.current.getContent();
      const cleaned = current.replace(/<!--wiki-image-upload:([^>]+)-->/g, (match, token: string) => jobs.has(token) ? "" : match);
      if (cleaned !== current) callbacks.current.onChange(cleaned);
      callbacks.current.onUploadsChange?.(false);
      if (initialized) instance.destroy();
    };
  }, [entryId]);

  useEffect(() => { control.current?.sync(callbacks.current.getContent()); }, [initialContent]);
  return <div className="wiki-vditor flex min-h-0 flex-1 flex-col">
    {metadataHost && createPortal(header, metadataHost)}
    {toolbarHost && createPortal(<div className="wiki-vditor-modes" role="group" aria-label="Wiki 编辑方式">
      {editingModes.map(mode => <button
        key={mode.value}
        type="button"
        data-wiki-mode={mode.value}
        aria-pressed={editingMode === mode.value}
        title={mode.description}
        disabled={!ready}
        onMouseDown={event => event.preventDefault()}
        onClick={() => control.current?.switchMode(mode.value)}
      >{mode.label}</button>)}
    </div>, toolbarHost)}
    {uploads.length > 0 && <div className="shrink-0 space-y-1 px-6 py-2 text-xs" aria-live="polite">
      {uploads.map(job => <div key={job.token} className="flex flex-wrap items-center gap-2">
        <span>{job.file.name || "粘贴的图片"}：{job.error ? `上传失败：${job.error}` : "上传中…"}</span>
        {job.error && <button type="button" className="underline" onClick={() => actions.current.retry(job.token)}>重试</button>}
        <button type="button" className="underline" onClick={() => actions.current.remove(job.token)}>移除占位</button>
      </div>)}
    </div>}
    <div ref={host} className="min-h-0 flex-1" aria-busy={!ready} />
  </div>;
}
