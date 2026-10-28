import { annotationPath, readerLocationId, beginReaderSession, isReaderSession, registerReaderSession, releaseReaderSession, findTextRanges, trapReaderFocus, restoreReaderFocus, type ReaderSearchTarget } from "./reader-target";
import { fetchBibFulltextRaw, gatewayRequest } from "@/gateway";
import { escapeHtml } from "@/lib/markdown";
import type { BibArticle } from "@/types";
import "./pdf-reader.css";
import "./txt-reader.css";
import { Toast } from "@/components/ui/toast";
import { ReaderAlertDialog } from "./reader-alert-dialog";
import { NoteComposer } from "./note-composer";
import {
  AnnotationInspector,
  type AnnotationPatch,
  type ReaderAnnotation,
} from "./annotation-inspector";

type ReaderPane = "outline" | "annotations" | "info" | "tags" | "search";

type OutlineEntry = {
  title: string;
  start: number;
  depth: number;
};

type SearchResult = {
  start: number;
  end: number;
};

type SelectionAnchor = {
  start: number;
  end: number;
  text: string;
  x: number;
  y: number;
};

export type TxtAppearance = {
  fontSize: number;
  fontFamily: "serif" | "sans";
  lineHeight: number;
  pageWidth: "narrow" | "normal" | "full";
  theme: "light" | "sepia" | "dark";
};

export const DEFAULT_TXT_APPEARANCE: TxtAppearance = {
  fontSize: 18,
  fontFamily: "serif",
  lineHeight: 1.6,
  pageWidth: "normal",
  theme: "light",
};

const READER_SIDEBAR_WIDTH_KEY = "autonomics.txt.sidebarWidth";
const READER_SIDEBAR_OPEN_KEY = "autonomics.txt.sidebarOpen";
const READER_APPEARANCE_KEY = "autonomics.txt.readingAppearance";
const MAX_SEARCH_RESULTS = 1000;

const READER_RAIL_ICONS: Record<ReaderPane, string> = {
  outline: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M4 6h16"/><path d="M4 12h10"/><path d="M4 18h13"/><circle cx="4" cy="6" r=".8" fill="currentColor"/><circle cx="4" cy="12" r=".8" fill="currentColor"/><circle cx="4" cy="18" r=".8" fill="currentColor"/></svg>`,
  annotations: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h9l5-5V5a2 2 0 0 0-2-2Z"/><path d="M8 8h7"/><path d="M8 12h5"/><path d="M15 21v-5h5"/></svg>`,
  info: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 10v7"/><path d="M12 7h.01"/></svg>`,
  tags: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L3 13V3h10l7.6 7.6a2 2 0 0 1 0 2.8Z"/><circle cx="7.5" cy="7.5" r="1"/></svg>`,
  search: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>`,
};

const READER_RAIL_LABELS: Record<ReaderPane, string> = {
  outline: "Outline",
  annotations: "Notes",
  info: "Info",
  tags: "Tags",
  search: "Search",
};

const THEME_DARK_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>`;
const THEME_LIGHT_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z"/></svg>`;

const TOOLBAR_ICONS: Record<string, string> = {
  sidebar: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>',
  previous: '<path d="m14 6-6 6 6 6"/>',
  next: '<path d="m10 6 6 6-6 6"/>',
  minus: '<path d="M5 12h14"/>',
  plus: '<path d="M5 12h14M12 5v14"/>',
  width: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="m8 10-2 2 2 2m8-4 2 2-2 2M6 12h12"/>',
  page: '<rect x="5" y="3" width="14" height="18" rx="2"/><path d="M9 7h6M9 17h6"/>',
  highlight: '<path d="m9 13 6-9 5 4-7 8-4-3ZM9 13l-4 5 3 2 5-4M3 21h8"/>',
  note: '<path d="M14 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-9M12 12l8-8M16 3l5 5"/>',
  copy: '<rect x="8" y="8" width="13" height="13" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/>',
  clear: '<path d="m16 3 5 5-11 11H5l-3-3L16 3ZM8 10l6 6M10 21h11"/>',
  appearance: '<path d="m3 18 5-12 5 12M5 14h6M16 10h5v8M21 14h-3a2 2 0 0 0 0 4h3"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
};

function toolbarButton(action: string, label: string, icon: string, text = "", extra = "") {
  return `<button type="button" data-action="${action}" title="${label}" aria-label="${label}" ${extra}>
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${TOOLBAR_ICONS[icon]}</svg>
    ${text ? `<span class="reader-tool-label">${text}</span>` : ""}
  </button>`;
}

export type TxtReaderHandle = {
  close: () => void;
};

export async function openTxtReader(options: {
  articleId: string;
  title: string;
  article?: BibArticle | null;
  bytes?: { bytes: Uint8Array; contentType: string };
  attachmentKey?: string | null;
  expectedHash?: string | null;
  search?: ReaderSearchTarget;
  sessionToken?: number;
}): Promise<TxtReaderHandle> {
  const token = options.sessionToken ?? beginReaderSession();
  const bytes = options.bytes ?? await fetchBibFulltextRaw(options.articleId, options.attachmentKey, options.expectedHash);
  if (!isReaderSession(token)) throw new DOMException("Reader opening superseded", "AbortError");
  const reader = await ZoteroStyleTxtReader.open(
    options.articleId,
    options.title,
    options.article ?? null,
    bytes, options.attachmentKey ?? undefined, options.search, token,
  );
  const handle: TxtReaderHandle = {
    close: () => reader.destroy(),
  };
  return handle;
}

/** Reader for plain-text full texts. The document is one reflowable text
 * node; annotations anchor to character offsets (`rects[].space === "char"`)
 * instead of PDF page space and are painted from live DOM ranges. */
class ZoteroStyleTxtReader {
  private raw = "";
  private textNode: Text | null = null;
  private outline: OutlineEntry[] = [];
  private outlinePositionCache: number[] | null = null;
  private searchResults: SearchResult[] = [];
  private annotations: ReaderAnnotation[] = [];
  private anchor: SelectionAnchor | null = null;
  private appearance: TxtAppearance = { ...DEFAULT_TXT_APPEARANCE };
  private destroyed = false;
  private frame = 0;
  private saveTimer: number | null = null;
  private readonly events = new AbortController();
  private resizeObserver: ResizeObserver | null = null;
  private returnFocus: Element | null = null;

  private root: HTMLDivElement;
  private toast: Toast;
  private alertDialog: ReaderAlertDialog;
  private noteComposer: NoteComposer;
  private container: HTMLDivElement;
  private article: HTMLElement;
  private overlay: HTMLDivElement;
  private flashLayer: HTMLDivElement;
  private annotationInspector: AnnotationInspector;
  private outlinePane: HTMLElement;
  private annotationPane: HTMLElement;
  private infoPane: HTMLElement;
  private tagsPane: HTMLElement;
  private searchPane: HTMLElement;
  private searchInput: HTMLInputElement;
  private selectionPopup: HTMLDivElement;
  private appearancePopup: HTMLDivElement;
  private statusElement: HTMLSpanElement;
  private progressElement: HTMLSpanElement;
  private fontSizeElement: HTMLSpanElement;
  private activePane: ReaderPane = "outline";
  private sidebarOpen = true;

  private constructor(
    private readonly articleId: string,
    private readonly title: string,
    private readonly bibArticle: BibArticle | null,
    private readonly sessionToken: number,
    private readonly attachmentKey?: string,
    private readonly initialSearch?: ReaderSearchTarget,
  ) {
    this.appearance = this.readAppearance();
    this.root = document.createElement("div");
    this.root.className = "zotero-pdf-reader";
    this.root.setAttribute("role","dialog");this.root.setAttribute("aria-modal","true");this.root.setAttribute("aria-label",title);this.root.tabIndex=-1;
    this.root.addEventListener("keydown",event=>trapReaderFocus(event,this.root));
    this.root.dataset.readerKind = "txt";
    this.root.innerHTML = `
      <div class="reader-backdrop" data-action="close-sidebar-backdrop"></div>
      <aside class="reader-sidebar-shell">
        <div class="reader-sidebar">
          <div class="reader-sidebar-header">
            <nav class="reader-rail" aria-label="Text sidebar">
              ${(["outline", "annotations", "info", "tags", "search"] as ReaderPane[]).map((pane) => `
                <button
                  type="button"
                  class="reader-rail-button${pane === this.activePane ? " active" : ""}"
                  data-pane="${pane}"
                  title="${READER_RAIL_LABELS[pane]}"
                  aria-label="${READER_RAIL_LABELS[pane]}"
                >
                  ${READER_RAIL_ICONS[pane]}
                </button>
              `).join("")}
            </nav>
            <button type="button" data-action="close-sidebar" title="Close sidebar">✕</button>
          </div>
          <div class="reader-sidebar-body">
            <div class="reader-sidebar-pane" data-pane="outline"></div>
            <div class="reader-sidebar-pane" data-pane="annotations" hidden></div>
            <div class="reader-sidebar-pane" data-pane="info" hidden>
              <div class="reader-metadata">
                ${this.metadataRow("Item Type", "Text Document")}
                ${this.metadataRow("Title", this.bibArticle?.title || this.title)}
                ${this.metadataRow("Authors", this.authorsText())}
                ${this.metadataRow("Publication", this.bibArticle?.journal)}
                ${this.metadataRow("Date", this.bibArticle?.year)}
                ${this.metadataRow("DOI", this.identifier("doi"))}
                ${this.metadataRow("PMID", this.identifier("pmid"))}
                ${this.metadataRow("Source", this.bibArticle?.source)}
                ${this.metadataRow("Record", this.bibArticle?.id)}
              </div>
            </div>
            <div class="reader-sidebar-pane" data-pane="tags" hidden>
              <div class="reader-tags" data-output="sidebar-tags"></div>
            </div>
            <div class="reader-sidebar-pane" data-pane="search" hidden>
              <input class="reader-search-input" data-input="search" placeholder="Search text" />
              <div data-output="search-results"></div>
            </div>
          </div>
        </div>
        <div class="reader-sidebar-resizer" data-action="resize-sidebar" role="separator" aria-orientation="vertical" aria-label="Resize sidebar"></div>
      </aside>
      <main class="reader-main">
        <header class="reader-toolbar" aria-label="Text controls">
          <div class="reader-toolbar-start">
            ${toolbarButton("sidebar", "Toggle sidebar", "sidebar")}
            <div class="reader-tool-group reader-pagination" role="group" aria-label="Section navigation">
              ${toolbarButton("section-previous", "Previous section", "previous")}
              <span class="reader-zoom-value" data-output="progress" aria-live="polite">0%</span>
              ${toolbarButton("section-next", "Next section", "next")}
            </div>
          </div>
          <div class="reader-tool-group reader-zoom-tools" role="group" aria-label="Text size and width">
            ${toolbarButton("font-out", "Smaller text", "minus")}
            <span class="reader-zoom-value" data-output="font-size" aria-live="polite">${this.appearance.fontSize} px</span>
            ${toolbarButton("font-in", "Larger text", "plus")}
            <span class="reader-tool-divider" aria-hidden="true"></span>
            ${toolbarButton("width-narrow", "Narrow column", "width", "Narrow")}
            ${toolbarButton("width-normal", "Normal column", "width", "Normal", 'aria-pressed="true"')}
            ${toolbarButton("width-full", "Full width", "page", "Full")}
          </div>
          <div class="reader-tool-group reader-tool-center" role="group" aria-label="Annotation tools">
            ${toolbarButton("tool-highlight", "Highlight selection", "highlight", "", 'class="reader-highlight-tool"')}
            ${toolbarButton("tool-note", "Add note", "note")}
            ${toolbarButton("copy-selection-toolbar", "Copy selection", "copy")}
            ${toolbarButton("tool-clear", "Clear selection", "clear")}
          </div>
          <div class="reader-tool-right" role="group" aria-label="Reading options">
            ${toolbarButton("text-options", "Text options", "appearance")}
            ${toolbarButton("open-search", "Search text", "search")}
            <button type="button" data-action="toggle-theme" title="Toggle dark mode" aria-label="Toggle dark mode">
              ${this.appearance.theme === "dark" ? THEME_DARK_ICON : THEME_LIGHT_ICON}
            </button>
            <span class="reader-tool-divider" aria-hidden="true"></span>
            ${toolbarButton("close", "Close reader", "close")}
          </div>
        </header>
        <div class="reader-reflow" data-output="document-container" tabindex="0" role="region" aria-label="Text document">
          <article class="reader-reflow-article reader-txt-document"></article>
        </div>
        <div class="reader-status">
          <span data-output="status">Opening text…</span>
        </div>
      </main>
      <div class="reader-selection-popup" data-output="selection-popup" hidden>
        <button type="button" data-color="yellow">Highlight</button>
        <button type="button" data-color="green">Green</button>
        <button type="button" data-color="red">Red</button>
        <button type="button" data-action="add-note">Note</button>
        <button type="button" data-action="copy-selection">Copy</button>
      </div>
      <div class="reader-appearance-popup" role="dialog" aria-label="Text appearance" hidden></div>
    `;
    document.body.append(this.root);
    this.toast = new Toast(this.root);
    this.alertDialog = new ReaderAlertDialog(this.root);
    this.noteComposer = new NoteComposer(this.root);

    this.container = this.select<HTMLDivElement>('[data-output="document-container"]');
    this.article = this.select(".reader-txt-document");
    this.overlay = document.createElement("div");
    this.overlay.className = "reader-reflow-overlays";
    this.flashLayer = document.createElement("div");
    this.flashLayer.className = "reader-reflow-overlays";
    this.container.append(this.overlay, this.flashLayer);
    this.outlinePane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="outline"]');
    this.annotationPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="annotations"]');
    this.infoPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="info"]');
    this.tagsPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="tags"]');
    this.searchPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="search"]');
    this.searchInput = this.select<HTMLInputElement>('.reader-sidebar-pane[data-pane="search"] [data-input="search"]');
    this.selectionPopup = this.select('[data-output="selection-popup"]');
    this.appearancePopup = this.select(".reader-appearance-popup");
    this.statusElement = this.select('[data-output="status"]');
    this.progressElement = this.select('[data-output="progress"]');
    this.fontSizeElement = this.select('[data-output="font-size"]');
    this.returnFocus = document.activeElement;

    this.annotationInspector = new AnnotationInspector(
      this.root,
      (id, patch) => this.updateAnnotation(id, patch),
      id => this.deleteAnnotation(id),
      () => {
        this.hideSelectionPopup();
        window.getSelection()?.removeAllRanges();
      },
    );
    this.renderAppearancePopup();

    const savedWidth = Number(window.localStorage.getItem(READER_SIDEBAR_WIDTH_KEY));
    if (Number.isFinite(savedWidth) && savedWidth >= 230) {
      this.root.style.setProperty("--reader-sidebar-width", `${Math.min(savedWidth, 560)}px`);
    }
    if (window.localStorage.getItem(READER_SIDEBAR_OPEN_KEY) === "0") {
      this.sidebarOpen = false;
      this.root.classList.add("sidebar-closed");
    }

    this.bindEvents();
  }

  static async open(
    articleId: string,
    title: string,
    bibArticle: BibArticle | null = null,
    fulltext?: { bytes: Uint8Array; contentType: string },
    attachmentKey?: string, initialSearch?: ReaderSearchTarget, sessionToken = beginReaderSession(),
  ): Promise<ZoteroStyleTxtReader> {
    const reader = new ZoteroStyleTxtReader(articleId, title, bibArticle, sessionToken, attachmentKey, initialSearch);
    if (!registerReaderSession(sessionToken, () => reader.destroy())) {reader.destroy();throw new DOMException("Reader opening superseded", "AbortError");}
    try {
      const raw = fulltext ?? await fetchBibFulltextRaw(articleId, attachmentKey);
      if (reader.destroyed) return reader;
      await reader.load(raw);
      if (!isReaderSession(sessionToken) || reader.destroyed) throw new DOMException("Reader closed", "AbortError");
      reader.root.focus({preventScroll:true});
      if (initialSearch) reader.revealSearch(initialSearch);
      return reader;
    } catch (error) {
      reader.destroy();
      throw error;
    }
  }

  private select<T extends Element = HTMLElement>(selector: string): T {
    const element = this.root.querySelector<T>(selector);
    if (!element) throw new Error(`Text reader element missing: ${selector}`);
    return element;
  }

  private async load(fulltext: { bytes: Uint8Array; contentType: string }) {
    if (/pdf/i.test(fulltext.contentType)) {
      throw new Error("The stored full text is a PDF; open it with the PDF reader");
    }
    this.setStatus("Decoding text…");
    const decoded = new TextDecoder("utf-8").decode(fulltext.bytes);
    this.raw = decoded.replace(/^﻿/, "").replace(/\r\n?/g, "\n");
    if (!this.raw.trim()) throw new Error("This file does not contain any text");
    // One text node keeps document offsets identical to DOM offsets, so
    // annotations and search never drift when the layout reflows.
    this.article.textContent = this.raw;
    this.textNode = this.article.firstChild as Text;

    this.outline = this.detectOutline();
    this.renderOutline();
    this.applyAppearance();
    this.resizeObserver = new ResizeObserver(() => {
      if (this.destroyed) return;
      this.outlinePositionCache = null;
      this.scheduleOverlayLayout();
    });
    this.resizeObserver.observe(this.article);
    await this.loadAnnotations();
    this.renderSidebarTags();
    if (!this.initialSearch) this.restoreReadingLocation();
    this.updateProgress();
    this.setStatus(
      `${this.raw.length.toLocaleString()} characters · ${this.outline.length.toLocaleString()} sections`,
    );
  }

  // -- Appearance ---------------------------------------------------------

  private readAppearance(): TxtAppearance {
    try {
      const value = JSON.parse(localStorage.getItem(READER_APPEARANCE_KEY) ?? "null");
      if (value && typeof value === "object") return this.validAppearance(value);
    } catch { /* A damaged preference must not prevent opening the text. */ }
    return { ...DEFAULT_TXT_APPEARANCE };
  }

  private validAppearance(value: Partial<TxtAppearance>): TxtAppearance {
    const base = DEFAULT_TXT_APPEARANCE;
    const number = (input: unknown, fallback: number, min: number, max: number) =>
      typeof input === "number" && Number.isFinite(input) ? Math.min(max, Math.max(min, input)) : fallback;
    return {
      fontSize: number(value.fontSize, base.fontSize, 12, 36),
      fontFamily: value.fontFamily === "sans" ? "sans" : "serif",
      lineHeight: number(value.lineHeight, base.lineHeight, .8, 2.4),
      pageWidth: ["narrow", "normal", "full"].includes(String(value.pageWidth)) ? value.pageWidth! : base.pageWidth,
      theme: ["light", "sepia", "dark"].includes(String(value.theme)) ? value.theme! : base.theme,
    };
  }

  private applyAppearance() {
    this.container.dataset.readingTheme = this.appearance.theme;
    this.article.dataset.pageWidth = this.appearance.pageWidth;
    this.article.style.setProperty("--reading-font-size", `${this.appearance.fontSize}px`);
    this.article.style.setProperty(
      "--reading-font-family",
      this.appearance.fontFamily === "serif"
        ? 'Georgia, "Noto Serif CJK SC", "Songti SC", serif'
        : "system-ui, sans-serif",
    );
    this.article.style.setProperty("--reading-line-height", String(this.appearance.lineHeight));
    this.fontSizeElement.textContent = `${this.appearance.fontSize} px`;
    this.outlinePositionCache = null;
    this.scheduleOverlayLayout();
    try {
      localStorage.setItem(READER_APPEARANCE_KEY, JSON.stringify(this.appearance));
    } catch { /* Optional preferences. */ }
  }

  private changeAppearance(change: Partial<TxtAppearance>) {
    this.appearance = this.validAppearance({ ...this.appearance, ...change });
    this.applyAppearance();
    this.updateWidthButtons();
    const themeButton = this.root.querySelector<HTMLButtonElement>('[data-action="toggle-theme"]');
    if (themeButton) {
      themeButton.innerHTML = this.appearance.theme === "dark" ? THEME_DARK_ICON : THEME_LIGHT_ICON;
    }
    this.hideSelectionPopup();
  }

  private updateWidthButtons() {
    for (const width of ["narrow", "normal", "full"] as const) {
      this.root.querySelector(`[data-action="width-${width}"]`)
        ?.setAttribute("aria-pressed", String(this.appearance.pageWidth === width));
    }
  }

  private renderAppearancePopup() {
    const value = this.appearance;
    this.appearancePopup.innerHTML = `
      <div class="reader-appearance-heading"><strong>Text appearance</strong><button type="button" data-appearance="close" aria-label="Close appearance">✕</button></div>
      <fieldset class="reader-reflow-settings"><legend>Typography</legend>
        <label>Font<select data-appearance="fontFamily"><option value="serif" ${value.fontFamily === "serif" ? "selected" : ""}>Serif</option><option value="sans" ${value.fontFamily === "sans" ? "selected" : ""}>Sans serif</option></select></label>
        ${([
          ["fontSize", "Font size", 12, 36, 1, value.fontSize],
          ["lineHeight", "Line height", .8, 2.4, .1, value.lineHeight],
        ] as const).map(([name, label, min, max, step, current]) => `<label>${label}<input type="range" aria-label="${label}" data-appearance="${name}" min="${min}" max="${max}" step="${step}" value="${current}" /><output data-appearance-output="${name}">${current}</output></label>`).join("")}
        <label>Page width<select data-appearance="pageWidth">${(["narrow", "normal", "full"] as const).map(width => `<option value="${width}" ${value.pageWidth === width ? "selected" : ""}>${width === "narrow" ? "Narrow" : width === "normal" ? "Normal" : "Full"}</option>`).join("")}</select></label>
        <div class="reader-reading-themes" role="group" aria-label="Reading theme">${(["light", "sepia", "dark"] as const).map(theme => `<button type="button" data-appearance-theme="${theme}" aria-pressed="${value.theme === theme}">${theme === "light" ? "Light" : theme === "sepia" ? "Sepia" : "Dark"}</button>`).join("")}</div>
        <button type="button" data-appearance="reset">Reset appearance</button>
      </fieldset>
      <p>Line breaks and spacing of the original text file are preserved.</p>`;
    this.appearancePopup.querySelector('[data-appearance="close"]')?.addEventListener("click", () => this.closeAppearancePopup());
    this.appearancePopup.querySelector('[data-appearance="reset"]')?.addEventListener("click", () => {
      this.changeAppearance({ ...DEFAULT_TXT_APPEARANCE });
      this.renderAppearancePopup();
    });
    this.appearancePopup.querySelectorAll<HTMLInputElement | HTMLSelectElement>("input[type=range],select").forEach(input => {
      input.addEventListener("input", () => {
        const name = input.dataset.appearance!;
        this.changeAppearance({ [name]: input instanceof HTMLInputElement ? Number(input.value) : input.value });
        const output = this.appearancePopup.querySelector(`[data-appearance-output="${name}"]`);
        if (output) output.textContent = input.value;
      });
    });
    this.appearancePopup.querySelectorAll<HTMLButtonElement>("[data-appearance-theme]").forEach(button => {
      button.addEventListener("click", () => {
        this.changeAppearance({ theme: button.dataset.appearanceTheme as TxtAppearance["theme"] });
        this.appearancePopup.querySelectorAll<HTMLButtonElement>("[data-appearance-theme]").forEach(node => node.setAttribute("aria-pressed", String(node === button)));
      });
    });
  }

  private toggleAppearancePopup() {
    const opening = this.appearancePopup.hidden;
    this.appearancePopup.hidden = !opening;
    this.root.querySelector('[data-action="text-options"]')?.setAttribute("aria-expanded", String(opening));
    if (opening) {
      this.renderAppearancePopup();
      const button = this.root.querySelector('[data-action="text-options"]')!.getBoundingClientRect();
      this.appearancePopup.style.left = `${Math.max(8, Math.min(button.right - 320, window.innerWidth - 328))}px`;
      this.appearancePopup.style.top = `${button.bottom + 8}px`;
      this.appearancePopup.querySelector<HTMLSelectElement>("[data-appearance]")?.focus();
    }
  }

  private closeAppearancePopup() {
    this.appearancePopup.hidden = true;
    this.root.querySelector('[data-action="text-options"]')?.setAttribute("aria-expanded", "false");
  }

  // -- Events -------------------------------------------------------------

  private bindEvents() {
    const action = (name: string, listener: () => void) => {
      this.root.querySelector(`[data-action="${name}"]`)?.addEventListener("click", listener);
    };
    const { signal } = this.events;

    action("close", () => this.destroy());
    action("section-previous", () => this.gotoSection(-1));
    action("section-next", () => this.gotoSection(1));
    action("font-out", () => this.changeAppearance({ fontSize: this.appearance.fontSize - 1 }));
    action("font-in", () => this.changeAppearance({ fontSize: this.appearance.fontSize + 1 }));
    action("width-narrow", () => this.changeAppearance({ pageWidth: "narrow" }));
    action("width-normal", () => this.changeAppearance({ pageWidth: "normal" }));
    action("width-full", () => this.changeAppearance({ pageWidth: "full" }));
    action("sidebar", () => this.setSidebarOpen(!this.sidebarOpen));
    action("close-sidebar", () => this.setSidebarOpen(false));
    action("close-sidebar-backdrop", () => this.setSidebarOpen(false));
    action("toggle-theme", () => {
      this.changeAppearance({ theme: this.appearance.theme === "dark" ? "light" : "dark" });
    });
    action("text-options", () => this.toggleAppearancePopup());
    action("open-search", () => {
      this.selectPane("search");
      this.setSidebarOpen(true);
      this.searchInput.focus();
    });
    action("tool-highlight", () => {
      if (this.anchor) void this.addHighlight("yellow");
      else this.setStatus("Select text first");
    });
    action("tool-note", () => void this.addNote());
    action("copy-selection-toolbar", () => void this.copySelection());
    action("tool-clear", () => {
      window.getSelection()?.removeAllRanges();
      this.hideSelectionPopup();
    });
    action("add-note", () => void this.addNote());
    action("copy-selection", () => void this.copySelection(true));

    this.selectionPopup.querySelectorAll<HTMLButtonElement>("[data-color]").forEach((button) => {
      button.addEventListener("click", () => void this.addHighlight(button.dataset.color ?? "yellow"));
    });

    this.searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") this.runSearch(this.searchInput.value.trim());
    });

    this.root
      .querySelectorAll<HTMLButtonElement>(".reader-rail-button")
      .forEach((button) => {
        button.addEventListener("click", () => {
          const pane = (button.dataset.pane ?? "outline") as ReaderPane;
          if (this.activePane === pane && this.sidebarOpen) {
            this.setSidebarOpen(false);
            return;
          }
          this.selectPane(pane);
          this.setSidebarOpen(true);
        });
      });

    this.root.querySelector<HTMLDivElement>('[data-action="resize-sidebar"]')?.addEventListener("pointerdown", (event: PointerEvent) => {
      if (!this.sidebarOpen) return;
      event.preventDefault();
      const resizer = event.currentTarget as HTMLElement;
      resizer.setPointerCapture(event.pointerId);
      resizer.classList.add("dragging");
      const move = (pointerEvent: PointerEvent) => {
        const width = Math.min(560, Math.max(230, pointerEvent.clientX));
        this.root.style.setProperty("--reader-sidebar-width", `${width}px`);
      };
      const finish = (pointerEvent: PointerEvent) => {
        const width = Math.min(560, Math.max(230, pointerEvent.clientX));
        this.root.style.setProperty("--reader-sidebar-width", `${width}px`);
        window.localStorage.setItem(READER_SIDEBAR_WIDTH_KEY, String(width));
        resizer.classList.remove("dragging");
        resizer.removeEventListener("pointermove", move);
        resizer.removeEventListener("pointerup", finish);
        resizer.removeEventListener("pointercancel", finish);
      };
      resizer.addEventListener("pointermove", move);
      resizer.addEventListener("pointerup", finish);
      resizer.addEventListener("pointercancel", finish);
    });

    this.container.addEventListener("scroll", () => {
      this.hideSelectionPopup();
      this.updateProgress();
      this.scheduleReadingSave();
      this.scheduleOverlayLayout();
    }, { signal, passive: true });
    this.container.addEventListener("mouseup", () => window.setTimeout(() => this.captureSelection(), 0), { signal });
    this.container.addEventListener("keyup", () => this.captureSelection(), { signal });
    this.container.addEventListener("mousedown", (event) => {
      const target = event.target as HTMLElement;
      if (!target.closest(".reader-selection-popup")) this.hideSelectionPopup();
    }, { signal });

    this.root.addEventListener("pointerdown", event => {
      const target = event.target as HTMLElement;
      if (!target.closest(".reader-appearance-popup, [data-action=text-options]")) this.closeAppearancePopup();
    }, { signal });
    this.root.addEventListener("wheel", this.handleWheelFont, { signal, passive: false });
    this.root.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        this.closeAppearancePopup();
        this.hideSelectionPopup();
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        this.selectPane("search");
        this.setSidebarOpen(true);
        this.searchInput.focus();
      }
    }, { signal });
  }

  private readonly handleWheelFont = (event: WheelEvent) => {
    if (!event.ctrlKey || event.deltaY === 0) return;
    event.preventDefault();
    this.changeAppearance({ fontSize: this.appearance.fontSize + (event.deltaY < 0 ? 1 : -1) });
  };

  private selectPane(name: string) {
    const pane = name as ReaderPane;
    this.activePane = pane;
    this.root.querySelectorAll<HTMLButtonElement>(".reader-rail-button").forEach((button) => {
      button.classList.toggle("active", button.dataset.pane === name);
    });
    this.outlinePane.hidden = name !== "outline";
    this.annotationPane.hidden = name !== "annotations";
    this.infoPane.hidden = name !== "info";
    this.tagsPane.hidden = name !== "tags";
    this.searchPane.hidden = name !== "search";
  }

  private setSidebarOpen(open: boolean) {
    this.sidebarOpen = open;
    this.root.classList.toggle("sidebar-closed", !open);
    window.localStorage.setItem(READER_SIDEBAR_OPEN_KEY, open ? "1" : "0");
  }

  // -- Document offsets ---------------------------------------------------

  private offsetAt(node: Node, offset: number): number | null {
    if (node === this.textNode) return offset;
    if (node === this.article) return offset === 0 ? 0 : this.raw.length;
    return null;
  }

  private rangeForSpan(start: number, end: number): Range | null {
    if (!this.textNode || end <= start) return null;
    const length = this.textNode.data.length;
    if (start >= length) return null;
    const range = document.createRange();
    range.setStart(this.textNode, start);
    range.setEnd(this.textNode, Math.min(end, length));
    return range;
  }

  private lineEnd(start: number): number {
    const newline = this.raw.indexOf("\n", start);
    return newline < 0 ? this.raw.length : newline;
  }

  // -- Selection ----------------------------------------------------------

  private captureSelection() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) {
      this.hideSelectionPopup();
      return;
    }
    const range = selection.getRangeAt(0);
    if (!this.article.contains(range.startContainer) || !this.article.contains(range.endContainer)) {
      this.hideSelectionPopup();
      return;
    }
    const start = this.offsetAt(range.startContainer, range.startOffset);
    const end = this.offsetAt(range.endContainer, range.endOffset);
    if (start === null || end === null || end <= start) {
      this.hideSelectionPopup();
      return;
    }
    const rects = range.getClientRects();
    const last = rects.length ? rects[rects.length - 1] : range.getBoundingClientRect();
    this.anchor = {
      start,
      end,
      text: this.raw.slice(start, end),
      x: last.left,
      y: last.bottom + 8,
    };
    this.selectionPopup.style.left = `${Math.max(8, Math.min(this.anchor.x, window.innerWidth - 280))}px`;
    this.selectionPopup.style.top = `${Math.max(8, Math.min(this.anchor.y, window.innerHeight - 60))}px`;
    this.selectionPopup.hidden = false;
  }

  private hideSelectionPopup() {
    this.selectionPopup.hidden = true;
    this.anchor = null;
  }

  private async copySelection(closePopup = false) {
    const text = this.anchor?.text;
    if (!text) {
      this.toast.show("Select text to copy first", "info");
      return;
    }
    try {
      await navigator.clipboard.writeText(text);
      if (this.destroyed) return;
      if (closePopup) this.hideSelectionPopup();
      this.toast.show("Copied to clipboard");
    } catch {
      if (!this.destroyed) this.toast.show("Unable to copy. Please try again.", "error");
    }
  }

  // -- Annotations --------------------------------------------------------

  private charSpan(annotation: ReaderAnnotation): { start: number; end: number } | null {
    const rect = annotation.rects?.find(item => item.space === "char");
    if (!rect) return null;
    const start = Math.round(rect.left);
    const length = Math.round(rect.width);
    if (!Number.isFinite(start) || !Number.isFinite(length) || length <= 0) return null;
    return { start: Math.max(0, start), end: Math.min(this.raw.length, start + length) };
  }

  private async loadAnnotations() {
    try {
      const response = await gatewayRequest(
        "GET",
        annotationPath(this.articleId, this.attachmentKey),
      );
      this.annotations = response.annotations ?? [];
      this.renderAnnotations();
      this.renderAnnotationList();
    } catch (error) {
      this.setStatus(`Annotations unavailable: ${String(error)}`);
    }
  }

  private renderAnnotations() {
    this.scheduleOverlayLayout();
    this.annotationInspector.setAnnotations(this.annotations);
  }

  private scheduleOverlayLayout() {
    if (this.frame || this.destroyed) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.drawAnnotations();
    });
  }

  private drawAnnotations() {
    this.overlay.replaceChildren();
    if (!this.textNode) return;
    const origin = this.container.getBoundingClientRect();
    const fragment = document.createDocumentFragment();
    for (const annotation of this.annotations) {
      if (annotation.kind !== "highlight") continue;
      const span = this.charSpan(annotation);
      if (!span) continue;
      const range = this.rangeForSpan(span.start, span.end);
      if (!range) continue;
      for (const rect of range.getClientRects()) {
        if (rect.width <= 0 || rect.height <= 0 || rect.bottom < origin.top || rect.top > origin.bottom) continue;
        const mark = document.createElement("div");
        mark.className = "reader-reflow-highlight";
        mark.dataset.annotationId = annotation.id;
        Object.assign(mark.style, {
          left: `${rect.left - origin.left + this.container.scrollLeft}px`,
          top: `${rect.top - origin.top + this.container.scrollTop}px`,
          width: `${rect.width}px`,
          height: `${rect.height}px`,
          background: annotation.color ?? "#ffd400",
        });
        fragment.append(mark);
      }
    }
    this.overlay.append(fragment);
  }

  private renderAnnotationList() {
    this.annotationPane.replaceChildren();
    if (!this.annotations.length) {
      this.annotationPane.innerHTML = `<div class="reader-empty">No annotations</div>`;
      return;
    }
    for (const annotation of [...this.annotations].reverse()) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "reader-annotation-list-item";
      button.dataset.annotationId = annotation.id;
      button.setAttribute("aria-pressed", "false");
      button.innerHTML = `
        <strong>Text</strong>
        <small>${escapeHtml(annotation.content.slice(0, 180))}</small>
        ${annotation.comment ? `<small class="reader-annotation-list-comment">${escapeHtml(annotation.comment)}</small>` : ""}
        ${annotation.tags?.length ? `<small>${annotation.tags.map((tag: string) => escapeHtml(tag)).join(" · ")}</small>` : ""}`;
      button.addEventListener("click", () => {
        const span = this.charSpan(annotation);
        if (span) this.flashSpan(span.start, span.end);
        if (!this.destroyed) this.annotationInspector.select(annotation.id);
      });
      this.annotationPane.append(button);
    }
  }

  private async addHighlight(color: string) {
    if (!this.anchor) return;
    const anchor = this.anchor;
    await gatewayRequest(
      "POST",
      annotationPath(this.articleId, this.attachmentKey),
      {
        content: anchor.text,
        kind: "highlight",
        page: 1,
        color,
        rects: [{ left: anchor.start, top: 0, width: anchor.end - anchor.start, height: 0, space: "char" }],
      },
    );
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    await this.loadAnnotations();
    this.renderSidebarTags();
  }

  private async addNote() {
    if (!this.anchor) return;
    const anchor = this.anchor;
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    this.noteComposer.open(anchor, async comment => {
      try {
        await gatewayRequest("POST", annotationPath(this.articleId, this.attachmentKey), {
          content: comment, kind: "comment", page: 1,
          rects: [{ left: anchor.start, top: 0, width: anchor.end - anchor.start, height: 0, space: "char" }],
        });
        await this.loadAnnotations();
        this.renderSidebarTags();
        return true;
      } catch (error) {
        this.toast.show(`Failed to save note: ${String(error)}`);
        return false;
      }
    });
  }

  private async updateAnnotation(id: string, patch: AnnotationPatch): Promise<ReaderAnnotation> {
    const response = await gatewayRequest("PATCH", `/bib/annotations/${encodeURIComponent(id)}`, patch);
    const annotation = response.annotation as ReaderAnnotation;
    if (!this.destroyed) {
      this.annotations = this.annotations.map(value => value.id === id ? annotation : value);
      this.renderAnnotations();
      this.renderAnnotationList();
    }
    return annotation;
  }

  private async deleteAnnotation(id: string) {
    if (!await this.alertDialog.open({ title: "Delete this annotation?", description: "The annotation will be permanently removed from the document.", actionLabel: "Delete", destructive: true })) return;
    await gatewayRequest("DELETE", `/bib/annotations/${encodeURIComponent(id)}`);
    await this.loadAnnotations();
    this.renderSidebarTags();
  }

  // -- Navigation ---------------------------------------------------------

  /** Scroll to a document span and paint a short-lived focus flash. */
  private flashSpan(start: number, end: number) {
    const range = this.rangeForSpan(start, end);
    if (!range) return;
    const rect = range.getBoundingClientRect();
    const origin = this.container.getBoundingClientRect();
    const target = this.container.scrollTop + rect.top - origin.top
      - this.container.clientHeight / 2 + rect.height / 2;
    this.container.scrollTop = Math.max(0, target);
    for (const line of range.getClientRects()) {
      if (line.width <= 0 || line.height <= 0) continue;
      const mark = document.createElement("div");
      mark.className = "reader-txt-flash";
      Object.assign(mark.style, {
        left: `${line.left - origin.left + this.container.scrollLeft}px`,
        top: `${line.top - origin.top + this.container.scrollTop}px`,
        width: `${line.width}px`,
        height: `${line.height}px`,
      });
      this.flashLayer.append(mark);
      window.setTimeout(() => mark.remove(), 1500);
    }
  }

  private detectOutline(): OutlineEntry[] {
    const entries: OutlineEntry[] = [];
    let offset = 0;
    for (const line of this.raw.split("\n")) {
      const start = offset;
      offset += line.length + 1;
      const trimmed = line.trim();
      if (trimmed.length < 2 || trimmed.length > 120) continue;
      const markdown = trimmed.match(/^(#{1,6})\s+(\S.*)$/);
      const numbered = trimmed.match(/^(\d+(?:\.\d+)*|[IVXLC]+\.)[.)]?\s+(\S.*)$/);
      let depth = 0;
      let title = "";
      if (markdown) {
        depth = markdown[1].length;
        title = markdown[2];
      } else if (numbered) {
        depth = numbered[1].replace(/\.$/, "").split(".").length + 1;
        title = trimmed;
      } else if (
        trimmed === trimmed.toUpperCase()
        && /[A-Z]/.test(trimmed)
        && !/[.,;:!?]$/.test(trimmed)
      ) {
        depth = 1;
        title = trimmed;
      } else continue;
      entries.push({ start, depth, title: title.trim() });
      if (entries.length >= 500) break;
    }
    return entries;
  }

  private renderOutline() {
    this.outlinePane.replaceChildren();
    for (const entry of this.outline) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = entry.title;
      button.style.paddingLeft = `${12 + (entry.depth - 1) * 14}px`;
      button.addEventListener("click", () => this.flashSpan(entry.start, this.lineEnd(entry.start)));
      this.outlinePane.append(button);
    }
    if (!this.outline.length) {
      this.outlinePane.innerHTML = `<div class="reader-empty">No outline detected</div>`;
    }
  }

  /** Vertical offsets of outline entries, measured lazily and cached until the
   * next layout change (appearance, sidebar, resize). */
  private outlinePositions(): number[] {
    if (this.outlinePositionCache) return this.outlinePositionCache;
    const origin = this.container.getBoundingClientRect();
    const top = this.container.scrollTop;
    this.outlinePositionCache = this.outline.map((entry) => {
      const range = this.rangeForSpan(entry.start, entry.start);
      if (!range) return 0;
      return range.getBoundingClientRect().top - origin.top + top;
    });
    return this.outlinePositionCache;
  }

  private gotoSection(delta: number) {
    if (!this.outline.length) {
      this.setStatus("No section headings were detected");
      return;
    }
    const positions = this.outlinePositions();
    const edge = this.container.scrollTop + 80;
    let current = 0;
    while (current < positions.length && positions[current] <= edge) current += 1;
    const target = Math.max(0, Math.min(this.outline.length - 1, current - 1 + delta));
    const entry = this.outline[target];
    this.flashSpan(entry.start, this.lineEnd(entry.start));
    this.setStatus(entry.title);
  }

  // -- Search -------------------------------------------------------------

  private revealSearch(target: ReaderSearchTarget) {
    if (target.annotationId) {
      const annotation = this.annotations.find(note => note.id === target.annotationId);
      if (!annotation) throw new Error("The annotation changed. Refresh search results.");
      const span = this.charSpan(annotation); if (span) this.flashSpan(span.start, span.end);
      this.selectPane("annotations"); this.setSidebarOpen(true); this.annotationInspector.select(target.annotationId); return;
    }
    if (target.query) {this.searchInput.value = target.query;this.selectPane("search");this.setSidebarOpen(true);this.searchInput.focus();this.runSearch(target.query,true);const first=this.searchResults[0];if(first)this.flashSpan(first.start,first.end);}
  }
  private runSearch(term: string, wordBoundary = false) {
    const results = findTextRanges(this.raw, term, wordBoundary, MAX_SEARCH_RESULTS);
    this.searchResults = results;
    this.renderSearchResults();
    this.setStatus(term
      ? `${results.length.toLocaleString()} ${results.length === 1 ? "match" : "matches"}`
      : "Type to search");
  }

  private snippet(start: number, end: number): string {
    const before = this.raw.slice(Math.max(0, start - 32), start).replace(/\s+/g, " ");
    const match = this.raw.slice(start, end).replace(/\s+/g, " ");
    const after = this.raw.slice(end, end + 48).replace(/\s+/g, " ");
    return `${before ? `…${before}` : ""}${match}${after ? `${after}…` : ""}`;
  }

  private renderSearchResults() {
    const output = this.searchPane.querySelector("[data-output=search-results]")!;
    output.replaceChildren();
    if (!this.searchResults.length) {
      output.innerHTML = `<div class="reader-empty">No matches</div>`;
      return;
    }
    this.searchResults.forEach((result, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.innerHTML = `<strong>Match ${(index + 1).toLocaleString()}</strong><small>${escapeHtml(this.snippet(result.start, result.end))}</small>`;
      button.addEventListener("click", () => this.flashSpan(result.start, result.end));
      output.append(button);
    });
  }

  // -- Misc ---------------------------------------------------------------

  private metadataRow(label: string, value?: string | number | null) {
    if (value === undefined || value === null || value === "") return "";
    return `<div><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`;
  }

  private authorsText() {
    return (this.bibArticle?.authors ?? [])
      .map((author) => author.name || [author.last_name, author.fore_name].filter(Boolean).join(", "))
      .filter(Boolean)
      .join("; ");
  }

  private identifier(kind: string) {
    return this.bibArticle?.identifiers?.find((item) => item.kind.toLowerCase() === kind)?.value ?? null;
  }

  private renderSidebarTags() {
    const tags = this.tagsPane.querySelector("[data-output=sidebar-tags]");
    if (tags) {
      const keywords = this.bibArticle?.keywords ?? [];
      tags.replaceChildren();
      if (!keywords.length) {
        tags.innerHTML = `<div class="reader-empty">No tags</div>`;
      } else {
        for (const keyword of keywords) {
          const badge = document.createElement("span");
          badge.className = "reader-tag";
          badge.textContent = keyword;
          tags.append(badge);
        }
      }
    }
  }

  private setStatus(text: string) {
    this.statusElement.textContent = text;
  }

  private updateProgress() {
    const max = this.container.scrollHeight - this.container.clientHeight;
    const fraction = max > 0 ? this.container.scrollTop / max : 0;
    this.progressElement.textContent = `${Math.round(fraction * 100)}%`;
  }

  private scheduleReadingSave() {
    if (this.saveTimer !== null) window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      this.saveReadingLocation();
    }, 300);
  }

  private saveReadingLocation() {
    try {
      const max = this.container.scrollHeight - this.container.clientHeight;
      localStorage.setItem(`autonomics.txt.readingLocation.${readerLocationId(this.articleId, this.attachmentKey)}`, JSON.stringify({
        fraction: max > 0 ? this.container.scrollTop / max : 0,
      }));
    } catch { /* Reading still works when local storage is unavailable. */ }
  }

  private restoreReadingLocation() {
    try {
      const saved = JSON.parse(localStorage.getItem(`autonomics.txt.readingLocation.${readerLocationId(this.articleId, this.attachmentKey)}`) ?? "null");
      if (!saved || !Number.isFinite(saved.fraction)) return;
      requestAnimationFrame(() => {
        if (this.destroyed) return;
        const max = this.container.scrollHeight - this.container.clientHeight;
        this.container.scrollTop = Math.max(0, Math.min(max, saved.fraction * max));
        this.updateProgress();
      });
    } catch { /* Invalid saved positions are ignored. */ }
  }

  destroy() {
    if (this.destroyed) return;
    this.saveReadingLocation();
    this.destroyed = true;
    releaseReaderSession(this.sessionToken);
    this.events.abort();
    this.toast.destroy();
    this.alertDialog.destroy();
    this.noteComposer.destroy();
    if (this.saveTimer !== null) window.clearTimeout(this.saveTimer);
    if (this.frame) cancelAnimationFrame(this.frame);
    this.annotationInspector.destroy();
    this.resizeObserver?.disconnect();
    restoreReaderFocus(this.returnFocus);
    this.root.remove();
  }
}
