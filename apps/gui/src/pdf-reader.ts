import { annotationPath, structurePath, readerLocationId, beginReaderSession, isReaderSession, registerReaderSession, releaseReaderSession, findTextRanges, trapReaderFocus, restoreReaderFocus, type ReaderSearchTarget } from "./reader-target";
import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy, PDFPageProxy, PageViewport, RenderTask } from "pdfjs-dist";
import { TextLayer } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { fetchBibFulltextRaw, gatewayRequest } from "./gateway";
import { escapeHtml } from "./markdown";
import type { BibArticle } from "./types";
import "./pdf-reader.css";
import type { ReadingTextRange } from "./reading-text-range";
import { highlightNoteIndicator } from "./highlight-note-indicator";
import { NoteComposer } from "./note-composer";
import { Toast } from "./components/ui/toast";
import { AlertDialog } from "./components/ui/alert-dialog";
import { PdfTextSelection } from "./pdf-text-selection";
import { AnnotationInspector, type AnnotationPatch, type ReaderAnnotation } from "./annotation-inspector";
import { openTxtReader } from "./txt-reader";
import { ReadingModeView, DEFAULT_READING_APPEARANCE, sourcePageRects, type ReadingAppearance } from "./reading-mode";
import type { StructuredDocumentText } from "./zotero-sdt.generated";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

type OutlineEntry = {
  title: string;
  page: number;
  depth: number;
};

type AnnotationTool = "pointer" | "highlight" | "note";

type FitMode = "width" | "page" | "custom";

type PageInfo = {
  width: number;
  height: number;
};

type PageView = {
  wrapper: HTMLDivElement;
  canvas: HTMLCanvasElement;
  text: HTMLDivElement;
  viewport: PageViewport | null;
  task: RenderTask | null;
  textTask: TextLayer | null;
  rendered: boolean;
  rendering: boolean;
  generation: number;
};

type SelectionAnchor = {
  page: number;
  text: string;
  rects: Array<PdfAnnotationRect>;
  x: number;
  y: number;
  extraPages?: Array<{ page: number; rects: PdfAnnotationRect[] }>;
};

type PdfAnnotationRect = {
  left: number;
  top: number;
  width: number;
  height: number;
  space?: "display" | "pdf";
  text_range?: ReadingTextRange;
};

type ReaderPane = "thumbnails" | "outline" | "annotations" | "info" | "tags" | "search";

type ReaderTheme = "light" | "dark";

const READER_SIDEBAR_WIDTH_KEY = "autonomics.pdf.sidebarWidth";
const READER_SIDEBAR_OPEN_KEY = "autonomics.pdf.sidebarOpen";
const READER_THEME_KEY = "autonomics.pdf.theme";

const READER_RAIL_ICONS: Record<ReaderPane, string> = {
  thumbnails: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18"/><path d="M9 9v12"/></svg>`,
  outline: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M4 6h16"/><path d="M4 12h10"/><path d="M4 18h13"/><circle cx="4" cy="6" r=".8" fill="currentColor"/><circle cx="4" cy="12" r=".8" fill="currentColor"/><circle cx="4" cy="18" r=".8" fill="currentColor"/></svg>`,
  annotations: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h9l5-5V5a2 2 0 0 0-2-2Z"/><path d="M8 8h7"/><path d="M8 12h5"/><path d="M15 21v-5h5"/></svg>`,
  info: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 10v7"/><path d="M12 7h.01"/></svg>`,
  tags: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L3 13V3h10l7.6 7.6a2 2 0 0 1 0 2.8Z"/><circle cx="7.5" cy="7.5" r="1"/></svg>`,
  search: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>`,
};

const READER_RAIL_LABELS: Record<ReaderPane, string> = {
  thumbnails: "Pages",
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
  book: '<path d="M12 6c-3-2-6-2-10-1v15c4-1 7-1 10 1 3-2 6-2 10-1V5c-4-1-7-1-10 1ZM12 6v15"/>',
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

export type PdfReaderHandle = {
  close: () => void;
};

export async function openPdfReader(options: {
  articleId: string;
  title: string;
  article?: BibArticle | null;
  attachmentKey?: string | null;
  expectedHash?: string | null;
  search?: ReaderSearchTarget;
  sessionToken?: number;
}): Promise<PdfReaderHandle> {
  const token = options.sessionToken ?? beginReaderSession();
  // Right-click entry points may not know the stored format; sniff the served
  // content type and hand plain text to the TXT reader instead.
  const raw = await fetchBibFulltextRaw(options.articleId, options.attachmentKey, options.expectedHash);
  if (!isReaderSession(token)) throw new DOMException("Reader opening superseded", "AbortError");
  if (!/pdf/i.test(raw.contentType)) {
    return openTxtReader({ ...options, bytes: raw, sessionToken: token });
  }
  const reader = await ZoteroStylePdfReader.open(
    options.articleId,
    options.title,
    options.article ?? null,
    raw,
    options.attachmentKey ?? undefined, options.search, token,
  );
  const handle: PdfReaderHandle = {
    close: () => reader.destroy(),
  };
  return handle;
}

class ZoteroStylePdfReader {
  private document: PDFDocumentProxy | null = null;
  private pageInfos = new Map<number, PageInfo>();
  private pageViews = new Map<number, PageView>();
  private thumbnails = new Set<number>();
  private currentPage = 1;
  private pageCount = 0;
  private zoom = 1;
  private rotation = 0;
  private fitMode: FitMode = "width";
  private destroyed = false;
  private pageObserver: IntersectionObserver | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private relayoutTimer: number | null = null;
  private layoutGeneration = 0;
  private wheelZoomFactor = 1;
  private wheelZoomAnchor: { x: number; y: number } | null = null;
  private wheelZoomRunning = false;

  private root: HTMLDivElement;
  private toast: Toast;
  private alertDialog: AlertDialog;
  private noteComposer: NoteComposer;
  private annotationTool: AnnotationTool = "pointer";
  private savingHighlights = new Set<SelectionAnchor>();
  private pageArea: HTMLDivElement;
  private textSelection: PdfTextSelection;
  private annotationInspector: AnnotationInspector;
  private pages: HTMLDivElement;
  private pageNumberInput: HTMLInputElement;
  private pageCountElement: HTMLSpanElement;
  private zoomLabel: HTMLSpanElement;
  private statusElement: HTMLSpanElement;
  private outlinePane: HTMLElement;
  private thumbnailsPane: HTMLElement;
  private annotationPane: HTMLElement;
  private infoPane: HTMLElement;
  private tagsPane: HTMLElement;
  private searchPane: HTMLElement;
  private searchInput: HTMLInputElement;
  private selectionPopup: HTMLDivElement;
  private annotations: any[] = [];
  private anchor: SelectionAnchor | null = null;
  private textCache = new Map<number, string>();
  private searchResults: Array<{ page: number; count: number }> = [];
  private activePane: ReaderPane = "thumbnails";
  private sidebarOpen = true;
  private pdfTheme: ReaderTheme;
  private readingArea: HTMLDivElement;
  private appearancePopup: HTMLDivElement;
  private readingView: ReadingModeView | null = null;
  private readingStructure: StructuredDocumentText | null = null;
  private readingEnabled = false;
  private readingLoading = false;
  private readingTransition: Promise<void> = Promise.resolve();
  private readingAppearance: ReadingAppearance = { ...DEFAULT_READING_APPEARANCE };
  private readingSaveTimer: number | null = null;
  private returnFocus: Element | null = null;

  private constructor(
    private readonly articleId: string,
    private readonly title: string,
    private readonly article: BibArticle | null,
    private readonly sessionToken: number,
    private readonly attachmentKey?: string,
    private readonly initialSearch?: ReaderSearchTarget,
  ) {
    this.pdfTheme = this.readTheme();
    this.root = document.createElement("div");
    this.root.className = "zotero-pdf-reader";
    this.root.setAttribute("role","dialog");this.root.setAttribute("aria-modal","true");this.root.setAttribute("aria-label",title);this.root.tabIndex=-1;
    this.root.addEventListener("keydown",event=>trapReaderFocus(event,this.root));
    this.root.dataset.pdfTheme = this.pdfTheme;
    this.root.innerHTML = `
      <aside class="reader-sidebar-shell">
        <nav class="reader-rail" aria-label="PDF sidebar">
          ${(["thumbnails", "outline", "annotations", "info", "tags", "search"] as ReaderPane[]).map((pane) => `
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
        <div class="reader-sidebar">
          <div class="reader-sidebar-header">
            <span data-output="sidebar-title">Pages</span>
            <button type="button" data-action="close-sidebar" title="Close sidebar">✕</button>
          </div>
          <div class="reader-sidebar-body">
            <div class="reader-sidebar-pane" data-pane="thumbnails"></div>
            <div class="reader-sidebar-pane" data-pane="outline" hidden></div>
            <div class="reader-sidebar-pane" data-pane="annotations" hidden></div>
            <div class="reader-sidebar-pane" data-pane="info" hidden>
              <div class="reader-metadata">
                ${this.metadataRow("Item Type", this.article?.source === "crossref" ? "Journal Article" : "Article")}
                ${this.metadataRow("Title", this.article?.title || this.title)}
                ${this.metadataRow("Authors", this.authorsText())}
                ${this.metadataRow("Publication", this.article?.journal)}
                ${this.metadataRow("Date", this.article?.year)}
                ${this.metadataRow("DOI", this.identifier("doi"))}
                ${this.metadataRow("PMID", this.identifier("pmid"))}
                ${this.metadataRow("Source", this.article?.source)}
                ${this.metadataRow("Record", this.article?.id)}
              </div>
            </div>
            <div class="reader-sidebar-pane" data-pane="tags" hidden>
              <div class="reader-tags" data-output="sidebar-tags"></div>
            </div>
            <div class="reader-sidebar-pane" data-pane="search" hidden>
              <input class="reader-search-input" data-input="search" placeholder="Search PDF" />
              <div data-output="search-results"></div>
            </div>
          </div>
        </div>
        <div class="reader-sidebar-resizer" data-action="resize-sidebar" role="separator" aria-orientation="vertical" aria-label="Resize sidebar"></div>
      </aside>
      <main class="reader-main">
        <header class="reader-toolbar" aria-label="PDF controls">
          <div class="reader-toolbar-start">
            ${toolbarButton("sidebar", "Toggle sidebar", "sidebar")}
            <div class="reader-tool-group reader-pagination" role="group" aria-label="Page navigation">
              ${toolbarButton("previous", "Previous page", "previous")}
              <div class="reader-page-field">
                <input data-input="page" type="number" min="1" value="1" aria-label="Page" />
                <span data-output="page-count">/ 0</span>
              </div>
              ${toolbarButton("next", "Next page", "next")}
            </div>
          </div>
          <div class="reader-tool-group reader-zoom-tools" role="group" aria-label="Zoom and fit">
            ${toolbarButton("zoom-out", "Zoom out", "minus")}
            <span class="reader-zoom-value" data-output="zoom" aria-live="polite">100%</span>
            ${toolbarButton("zoom-in", "Zoom in", "plus")}
            <span class="reader-tool-divider" aria-hidden="true"></span>
            ${toolbarButton("fit-width", "Fit width", "width", "Width", 'aria-pressed="true"')}
            ${toolbarButton("fit-page", "Fit page", "page", "Page", 'aria-pressed="false"')}
          </div>
          <div class="reader-tool-group reader-tool-center" role="group" aria-label="Annotation tools">
            ${toolbarButton("tool-highlight", "Highlight text", "highlight", "", 'class="reader-highlight-tool" aria-pressed="false"')}
            ${toolbarButton("tool-note", "Add note", "note", "", 'aria-pressed="false"')}
            ${toolbarButton("copy-selection-toolbar", "Copy selection", "copy")}
            ${toolbarButton("tool-clear", "Clear selection", "clear")}
          </div>
          <div class="reader-tool-right" role="group" aria-label="Reading options">
            ${toolbarButton("reading-mode", "Reading Mode (Alt+R)", "book", "Read", 'aria-pressed="false"')}
            ${toolbarButton("text-options", "Text options", "appearance")}
            ${toolbarButton("open-search", "Search PDF", "search")}
            <button type="button" data-action="toggle-theme" title="Toggle dark mode" aria-label="Toggle dark mode">
              ${this.pdfTheme === "dark" ? THEME_DARK_ICON : THEME_LIGHT_ICON}
            </button>
            <span class="reader-tool-divider" aria-hidden="true"></span>
            ${toolbarButton("close", "Close reader", "close")}
          </div>
        </header>
        <div class="reader-page-area">
          <div class="reader-pages"></div>
        <div class="reader-selection-popup" data-output="selection-popup" hidden>
          <button type="button" data-color="yellow">Highlight</button>
          <button type="button" data-color="green">Green</button>
          <button type="button" data-color="red">Red</button>
          <button type="button" data-action="add-note">Note</button>
          <button type="button" data-action="copy-selection">Copy</button>
        </div>
          <div class="reader-status">
            <span data-output="status">Opening PDF…</span>
          </div>
        </div>
      </main>
    `;
    document.body.append(this.root);
    this.toast = new Toast(this.root);
    this.alertDialog = new AlertDialog(this.root);
    this.noteComposer = new NoteComposer(this.root);

    this.pageArea = this.select(".reader-page-area");
    this.textSelection = new PdfTextSelection(this.pageArea);
    this.pages = this.select(".reader-pages");
    this.pageNumberInput = this.select<HTMLInputElement>('[data-input="page"]');
    this.pageCountElement = this.select('[data-output="page-count"]');
    this.zoomLabel = this.select('[data-output="zoom"]');
    this.statusElement = this.select('[data-output="status"]');
    this.outlinePane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="outline"]');
    this.thumbnailsPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="thumbnails"]');
    this.annotationPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="annotations"]');
    this.infoPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="info"]');
    this.tagsPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="tags"]');
    this.searchPane = this.select<HTMLElement>('.reader-sidebar-pane[data-pane="search"]');
    this.searchInput = this.select<HTMLInputElement>('.reader-sidebar-pane[data-pane="search"] [data-input="search"]');
    this.selectionPopup = this.select('[data-output="selection-popup"]');
    this.applyTheme();
    this.returnFocus = document.activeElement;
    this.root.append(this.selectionPopup);
    this.readingArea = document.createElement("div");
    this.readingArea.className = "reader-reflow";
    this.readingArea.hidden = true;
    this.readingArea.tabIndex = 0;
    this.readingArea.setAttribute("role", "region");
    this.readingArea.setAttribute("aria-label", "Reading Mode document");
    this.root.querySelector(".reader-main")!.append(this.readingArea, this.root.querySelector(".reader-status")!);
    this.appearancePopup = document.createElement("div");
    this.appearancePopup.className = "reader-appearance-popup";
    this.appearancePopup.hidden = true;
    this.appearancePopup.setAttribute("role", "dialog");
    this.appearancePopup.setAttribute("aria-label", "Reading appearance");
    this.root.append(this.appearancePopup);
    this.annotationInspector = new AnnotationInspector(this.root,
      (id, patch) => this.updateAnnotation(id, patch),
      id => this.deleteAnnotation(id),
      () => { this.hideSelectionPopup(); window.getSelection()?.removeAllRanges(); });
    this.restoreReadingAppearance();
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
    article: BibArticle | null = null,
    fulltext: { bytes: Uint8Array; contentType: string },
    attachmentKey?: string, initialSearch?: ReaderSearchTarget, sessionToken = beginReaderSession(),
  ): Promise<ZoteroStylePdfReader> {
    const reader = new ZoteroStylePdfReader(articleId, title, article, sessionToken, attachmentKey, initialSearch);
    if (!registerReaderSession(sessionToken, () => reader.destroy())) {reader.destroy();throw new DOMException("Reader opening superseded", "AbortError");}
    try {
      if (reader.destroyed) return reader;
      await reader.load(fulltext.bytes);
      if (!isReaderSession(sessionToken) || reader.destroyed) throw new DOMException("Reader closed", "AbortError");
      reader.root.focus({preventScroll:true});
      if (initialSearch) await reader.revealSearch(initialSearch);
      return reader;
    } catch (error) {
      reader.destroy();
      throw error;
    }
  }

  private select<T extends Element = HTMLElement>(selector: string): T {
    const element = this.root.querySelector<T>(selector);
    if (!element) throw new Error(`PDF reader element missing: ${selector}`);
    return element;
  }

  private restoreReadingAppearance() {
    try {
      const value = JSON.parse(localStorage.getItem("autonomics.pdf.readingAppearance") ?? "null");
      if (value && typeof value === "object") this.readingAppearance = this.validReadingAppearance(value);
    } catch { /* A damaged preference must not prevent opening the PDF. */ }
  }

  private validReadingAppearance(value: Partial<ReadingAppearance>): ReadingAppearance {
    const base = DEFAULT_READING_APPEARANCE;
    const number = (input: unknown, fallback: number, min: number, max: number) =>
      typeof input === "number" && Number.isFinite(input) ? Math.min(max, Math.max(min, input)) : fallback;
    return {
      fontSize: number(value.fontSize, base.fontSize, 12, 36),
      fontFamily: value.fontFamily === "sans" ? "sans" : "serif",
      lineHeight: number(value.lineHeight, base.lineHeight, .8, 2),
      wordSpacing: number(value.wordSpacing, base.wordSpacing, -50, 100),
      letterSpacing: number(value.letterSpacing, base.letterSpacing, -.05, .1),
      pageWidth: ["narrow", "normal", "full"].includes(String(value.pageWidth)) ? value.pageWidth! : base.pageWidth,
      theme: ["light", "sepia", "dark"].includes(String(value.theme)) ? value.theme! : base.theme,
    };
  }

  private renderAppearancePopup() {
    const value = this.readingAppearance;
    this.appearancePopup.innerHTML = `
      <div class="reader-appearance-heading"><strong>Reading appearance</strong><button type="button" data-appearance="close" aria-label="Close appearance">✕</button></div>
      <label class="reader-reading-switch"><span>Reading Mode</span><input type="checkbox" data-appearance="enabled" ${this.readingEnabled ? "checked" : ""} /></label>
      <fieldset class="reader-reflow-settings"><legend>Reflowable text</legend>
        <label>Font<select data-appearance="fontFamily"><option value="serif" ${value.fontFamily === "serif" ? "selected" : ""}>Serif</option><option value="sans" ${value.fontFamily === "sans" ? "selected" : ""}>Sans serif</option></select></label>
        ${([
          ["fontSize", "Font size", 12, 36, 1, value.fontSize],
          ["lineHeight", "Line height", .8, 2, .4, value.lineHeight],
          ["wordSpacing", "Word spacing", -50, 100, 10, value.wordSpacing],
          ["letterSpacing", "Letter spacing", -.05, .1, .01, value.letterSpacing],
        ] as const).map(([name, label, min, max, step, current]) => `<label>${label}<input type="range" aria-label="${label}" data-appearance="${name}" min="${min}" max="${max}" step="${step}" value="${current}" /><output data-appearance-output="${name}">${current}</output></label>`).join("")}
        <label>Page width<select data-appearance="pageWidth">${(["narrow", "normal", "full"] as const).map(width => `<option value="${width}" ${value.pageWidth === width ? "selected" : ""}>${width === "narrow" ? "Narrow" : width === "normal" ? "Normal" : "Full"}</option>`).join("")}</select></label>
        <div class="reader-reading-themes" role="group" aria-label="Reading theme">${(["light", "sepia", "dark"] as const).map(theme => `<button type="button" data-appearance-theme="${theme}" aria-pressed="${value.theme === theme}">${theme === "light" ? "Light" : theme === "sepia" ? "Sepia" : "Dark"}</button>`).join("")}</div>
        <button type="button" data-appearance="reset">Reset appearance</button>
      </fieldset>
      <p>Text reflows to fit the window. Figures and formulas retain their original PDF appearance.</p>`;
    this.appearancePopup.querySelector('[data-appearance="close"]')?.addEventListener("click", () => this.closeAppearancePopup());
    this.appearancePopup.querySelector('[data-appearance="enabled"]')?.addEventListener("change", () => void this.toggleReadingMode());
    this.appearancePopup.querySelector('[data-appearance="reset"]')?.addEventListener("click", () => {
      this.changeReadingAppearance({ ...DEFAULT_READING_APPEARANCE }); this.renderAppearancePopup();
    });
    this.appearancePopup.querySelectorAll<HTMLInputElement | HTMLSelectElement>("input[type=range],select").forEach(input => {
      input.addEventListener("input", () => {
        const name = input.dataset.appearance!;
        this.changeReadingAppearance({ [name]: input instanceof HTMLInputElement ? Number(input.value) : input.value });
        const output = this.appearancePopup.querySelector(`[data-appearance-output="${name}"]`);
        if (output) output.textContent = input.value;
      });
    });
    this.appearancePopup.querySelectorAll<HTMLButtonElement>("[data-appearance-theme]").forEach(button => {
      button.addEventListener("click", () => {
        this.changeReadingAppearance({ theme: button.dataset.appearanceTheme as ReadingAppearance["theme"] });
        this.appearancePopup.querySelectorAll<HTMLButtonElement>("[data-appearance-theme]").forEach(node => node.setAttribute("aria-pressed", String(node === button)));
      });
    });
  }

  private changeReadingAppearance(change: Partial<ReadingAppearance>) {
    const anchor = this.readingView?.captureAnchor() ?? null;
    this.readingAppearance = this.validReadingAppearance({ ...this.readingAppearance, ...change });
    this.readingView?.setAppearance(this.readingAppearance);
    this.readingView?.restoreAnchor(anchor);
    if (this.readingEnabled) this.zoomLabel.textContent = `${this.readingAppearance.fontSize} px`;
    this.hideSelectionPopup();
    try { localStorage.setItem("autonomics.pdf.readingAppearance", JSON.stringify(this.readingAppearance)); } catch { /* Optional preferences. */ }
    this.scheduleReadingSave();
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
      this.appearancePopup.querySelector<HTMLInputElement>('[data-appearance="enabled"]')?.focus();
    }
  }

  private closeAppearancePopup() {
    this.appearancePopup.hidden = true;
    this.root.querySelector('[data-action="text-options"]')?.setAttribute("aria-expanded", "false");
  }

  private toggleReadingMode(): Promise<void> {
    this.readingTransition = this.readingTransition.then(() => this.setReadingMode(!this.readingEnabled));
    return this.readingTransition;
  }

  private async setReadingMode(enabled: boolean) {
    if (this.destroyed || !this.document || this.readingLoading) return;
    const button = this.root.querySelector<HTMLButtonElement>('[data-action="reading-mode"]')!;
    this.hideSelectionPopup();
    this.annotationInspector.close();
    window.getSelection()?.removeAllRanges();
    try {
      if (enabled) {
        this.readingLoading = true;
        button.disabled = true; button.setAttribute("aria-busy", "true");
        button.textContent = "Preparing…";
        this.setStatus("Extracting structured text with Zotero…");
        const pdfPage = this.currentPage;
        const view = this.pageViews.get(pdfPage);
        const bounds = view?.wrapper.getBoundingClientRect();
        const area = this.pageArea.getBoundingClientRect();
        const pdfTop = bounds && view?.viewport
          ? view.viewport.convertToPdfPoint(0, Math.max(0, area.top - bounds.top))[1] : undefined;
        if (!this.readingStructure) {
          const response = await gatewayRequest("POST", structurePath(this.articleId, this.attachmentKey));
          if (this.destroyed) return;
          const structure = response?.structure;
          if (!structure || structure.metadata?.processor?.type !== "pdf" || !Array.isArray(structure.content) || !Array.isArray(structure.catalog?.pages)) {
            throw new Error("The extractor did not return a Zotero SDT document");
          }
          if (!structure.content.length) throw new Error("No readable content was found in this PDF");
          this.readingStructure = structure;
        }
        this.readingEnabled = true;
        this.pageArea.hidden = true; this.readingArea.hidden = false;
        this.root.dataset.readingMode = "true";
        this.readingView = new ReadingModeView(this.readingArea, this.readingStructure!, this.document, page => {
          if (page !== this.currentPage) this.setCurrentPage(page, true);
        }, id => this.openHighlightNote(id));
        this.readingView.setAppearance(this.readingAppearance);
        this.readingView.setAnnotations(this.annotations);
        this.readingView.gotoPage(pdfPage, pdfTop);
        try {
          const saved = JSON.parse(localStorage.getItem(`autonomics.pdf.readingLocation.${readerLocationId(this.articleId, this.attachmentKey)}`) ?? "null");
          if (saved?.enabled && saved.hash === this.readingStructure!.metadata.source.hash && saved.anchor && /^\d+(\.\d+)*$/.test(saved.anchor.ref) && Number.isFinite(saved.anchor.fraction)) {
            this.readingView.restoreAnchor(saved.anchor);
          }
        } catch { /* Invalid saved positions are ignored. */ }
        this.zoomLabel.textContent = `${this.readingAppearance.fontSize} px`;
        this.renderReadingOutline();
        this.readingArea.focus({ preventScroll: true });
        this.setStatus("Reading Mode · Zotero structured text");
      } else {
        const anchor = this.readingView?.captureAnchor(this.readingArea.clientHeight / 2);
        const block = anchor && this.readingStructure?.content[Number(anchor.ref.split(".")[0])];
        const source = sourcePageRects(block || undefined)[0];
        this.saveReadingLocation();
        this.readingEnabled = false;
        this.root.dataset.readingMode = "false";
        this.readingView?.destroy(); this.readingView = null;
        this.readingArea.hidden = true; this.pageArea.hidden = false;
        await this.refit();
        if (this.destroyed) return;
        const page = source ? source[0] + 1 : this.currentPage;
        const proxy = await this.document.getPage(page);
        if (this.destroyed) return;
        const viewport = proxy.getViewport({ scale: this.zoom, rotation: (proxy.rotate + this.rotation) % 360 });
        const wrapper = this.pageViews.get(page)?.wrapper;
        if (wrapper) {
          const y = source ? viewport.convertToViewportPoint(source[1], source[4])[1] : 0;
          this.pageArea.scrollTop += wrapper.getBoundingClientRect().top - this.pageArea.getBoundingClientRect().top + y - 24;
        }
        this.setCurrentPage(page, true);
        await this.loadOutline();
        this.pageArea.tabIndex = 0; this.pageArea.focus({ preventScroll: true });
        this.setStatus(`${page} / ${this.pageCount}`);
      }
      this.root.querySelectorAll<HTMLButtonElement>('[data-action="fit-width"],[data-action="fit-page"]').forEach(node => node.disabled = enabled);
      this.saveReadingLocation();
    } catch (error) {
      if (!this.destroyed) this.setStatus(`Reading Mode unavailable: ${String(error)}`);
      if (!this.readingView) {
        this.readingEnabled = false; this.pageArea.hidden = false; this.readingArea.hidden = true;
        this.root.dataset.readingMode = "false";
      }
    } finally {
      this.readingLoading = false;
      button.disabled = false; button.removeAttribute("aria-busy");
      const readingLabel = this.readingEnabled ? "Original PDF" : "Reading Mode (Alt+R)";
      button.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${TOOLBAR_ICONS[this.readingEnabled ? "page" : "book"]}</svg><span class="reader-tool-label">${this.readingEnabled ? "PDF" : "Read"}</span>`;
      button.title = readingLabel;
      button.setAttribute("aria-label", readingLabel);
      button.setAttribute("aria-pressed", String(this.readingEnabled));
      this.renderAppearancePopup();
    }
  }

  private renderReadingOutline() {
    this.outlinePane.replaceChildren();
    const walk = (entries: StructuredDocumentText["catalog"]["outline"], depth: number) => {
      for (const entry of entries) {
        const button = document.createElement("button"); button.type = "button";
        button.textContent = entry.title; button.style.paddingLeft = `${12 + depth * 14}px`;
        button.addEventListener("click", () => {
          if (entry.ref) this.readingView?.restoreAnchor({ ref: entry.ref.join("."), fraction: 0 });
        });
        this.outlinePane.append(button);
        if (entry.children) walk(entry.children, depth + 1);
      }
    };
    walk(this.readingStructure!.catalog.outline, 0);
    if (!this.outlinePane.childElementCount) this.outlinePane.innerHTML = '<div class="reader-empty">No outline</div>';
  }

  private captureReadingSelection(range: Range) {
    const selected = this.readingView?.selection(range);
    if (!selected || !selected.text) { this.hideSelectionPopup(); return; }
    const targets = [{ page: selected.position.pageIndex, rects: selected.position.rects }];
    if (selected.position.nextPageRects?.length) targets.push({ page: selected.position.pageIndex + 1, rects: selected.position.nextPageRects });
    const pages = targets.map(target => {
      const [x0, y0, x1, y1] = this.readingStructure!.catalog.pages[target.page].viewRect!;
      const textRange = targets.length === 1 ? selected.textRange : this.readingView!.textRangeOnPage(selected.textRange, { pageIndex: target.page, rects: target.rects });
      return { page: target.page + 1, rects: target.rects.map((rect, index) => ({
        ...(index === 0 && textRange ? { text_range: textRange } : {}),
        left: (rect[0] - x0) / (x1 - x0), top: (rect[1] - y0) / (y1 - y0),
        width: (rect[2] - rect[0]) / (x1 - x0), height: (rect[3] - rect[1]) / (y1 - y0), space: "pdf" as const,
      })) };
    });
    this.anchor = { ...pages[0], extraPages: pages.slice(1), text: selected.text, x: selected.rect.left, y: selected.rect.bottom + 8 };
    this.selectionPopup.style.left = `${Math.max(8, Math.min(this.anchor.x, window.innerWidth - 280))}px`;
    this.selectionPopup.style.top = `${Math.max(8, Math.min(this.anchor.y, window.innerHeight - 60))}px`;
    this.selectionPopup.hidden = false;
  }

  private scheduleReadingSave() {
    if (this.readingSaveTimer !== null) clearTimeout(this.readingSaveTimer);
    this.readingSaveTimer = window.setTimeout(() => { this.readingSaveTimer = null; this.saveReadingLocation(); }, 300);
  }

  private saveReadingLocation() {
    if (!this.readingStructure) return;
    try {
      localStorage.setItem(`autonomics.pdf.readingLocation.${readerLocationId(this.articleId, this.attachmentKey)}`, JSON.stringify({
        enabled: this.readingEnabled, page: this.currentPage,
        hash: this.readingStructure.metadata.source.hash,
        anchor: this.readingView?.captureAnchor() ?? null,
      }));
    } catch { /* Reading still works when local storage is unavailable. */ }
  }

  private readTheme(): ReaderTheme {
    const saved = window.localStorage.getItem(READER_THEME_KEY);
    if (saved === "light" || saved === "dark") return saved;
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  private applyTheme() {
    this.root.dataset.pdfTheme = this.pdfTheme;
    const button = this.root.querySelector<HTMLButtonElement>('[data-action="toggle-theme"]');
    if (button) {
      button.innerHTML = this.pdfTheme === "dark" ? THEME_DARK_ICON : THEME_LIGHT_ICON;
      button.title = this.pdfTheme === "dark" ? "Switch to light mode" : "Switch to dark mode";
      button.setAttribute("aria-label", button.title);
    }
  }

  private toggleTheme() {
    this.pdfTheme = this.pdfTheme === "dark" ? "light" : "dark";
    window.localStorage.setItem(READER_THEME_KEY, this.pdfTheme);
    this.applyTheme();
  }

  private bindEvents() {
    const action = (name: string, listener: () => void) => {
      this.root.querySelector(`[data-action="${name}"]`)?.addEventListener("click", listener);
    };

    action("close", () => this.destroy());
    action("previous", () => void this.gotoPage(this.currentPage - 1));
    action("next", () => void this.gotoPage(this.currentPage + 1));
    action("zoom-out", () => void this.setZoom(this.zoom / 1.2));
    action("zoom-in", () => void this.setZoom(this.zoom * 1.2));
    action("fit-width", () => {
      this.fitMode = "width";
      void this.refit();
    });
    action("fit-page", () => {
      this.fitMode = "page";
      void this.refit();
    });
    action("rotate", () => {
      this.rotation = (this.rotation + 90) % 360;
      void this.refit();
    });
    action("sidebar", () => this.setSidebarOpen(!this.sidebarOpen));
    action("toggle-theme", () => {
      if (this.readingEnabled) this.changeReadingAppearance({ theme: this.readingAppearance.theme === "dark" ? "light" : "dark" });
      else this.toggleTheme();
    });
    action("reading-mode", () => void this.toggleReadingMode());
    action("text-options", () => this.toggleAppearancePopup());
    action("close-sidebar", () => this.setSidebarOpen(false));
    action("tool-highlight", () => {
      const selected = this.anchor;
      this.setAnnotationTool(this.annotationTool === "highlight" ? "pointer" : "highlight");
      if (this.annotationTool === "highlight" && selected) void this.addHighlight("#ffd400");
    });
    action("tool-note", () => {
      const selected = this.anchor;
      this.setAnnotationTool(this.annotationTool === "note" ? "pointer" : "note");
      if (this.annotationTool === "note" && selected) this.addNote(selected);
    });
    action("copy-selection-toolbar", () => void this.copySelection());
    action("tool-clear", () => {
      this.setAnnotationTool("pointer");
      window.getSelection()?.removeAllRanges();
      this.hideSelectionPopup();
    });
    action("open-search", () => {
      this.selectPane("search");
      this.setSidebarOpen(true);
      this.searchInput.focus();
    });
    action("open-external", () => {
      const href = this.externalHref();
      if (href) window.open(href, "_blank", "noopener");
    });
    action("close-sidebar", () => this.setSidebarOpen(false));
    action("add-note", () => this.addNote());
    action("copy-selection", () => void this.copySelection(true));

    this.selectionPopup.querySelectorAll<HTMLButtonElement>("[data-color]").forEach((button) => {
      button.addEventListener("click", () => void this.addHighlight(button.dataset.color ?? "yellow"));
    });

    this.root.querySelector<HTMLInputElement>('[data-input="toolbar-search"]')?.addEventListener("keydown", (event: KeyboardEvent) => {
      if (event.key === "Enter") {
        const value = (event.target as HTMLInputElement).value.trim();
        void this.searchPdf(value);
      }
    });
    this.searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") void this.searchPdf(this.searchInput.value.trim());
    });

    this.pageNumberInput.addEventListener("change", () => {
      void this.gotoPage(Number(this.pageNumberInput.value));
    });

    this.root
      .querySelectorAll<HTMLButtonElement>(".reader-rail-button")
      .forEach((button) => {
        button.addEventListener("click", () => {
          const pane = (button.dataset.pane ?? "thumbnails") as ReaderPane;
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
        const width = Math.min(560, Math.max(230, pointerEvent.clientX - 58));
        this.root.style.setProperty("--reader-sidebar-width", `${width}px`);
      };
      const finish = (pointerEvent: PointerEvent) => {
        const width = Math.min(560, Math.max(230, pointerEvent.clientX - 58));
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

    this.pageArea.addEventListener("scroll", () => this.updateCurrentFromScroll());
    this.pageArea.addEventListener("pointerdown", event => this.placeNote(event));
    this.pageArea.addEventListener("mouseup", event => {
      if (event.button === 0 && this.annotationTool !== "note") window.setTimeout(() => this.finishTextSelection(), 0);
    });
    this.pageArea.addEventListener("mousedown", (event) => {
      const target = event.target as HTMLElement;
      if (!target.closest(".reader-selection-popup") && !target.closest(".reader-annotation")) {
        this.hideSelectionPopup();
      }
    });
    this.pageArea.addEventListener("keydown", (event) => {
      if (event.key === "ArrowLeft" || event.key === "PageUp") {
        event.preventDefault();
        void this.gotoPage(this.currentPage - 1);
      } else if (event.key === "ArrowRight" || event.key === "PageDown") {
        event.preventDefault();
        void this.gotoPage(this.currentPage + 1);
      }
    });

    this.root.addEventListener("wheel", this.handleWheelZoom, { passive: false });
    document.addEventListener("keydown", this.handleToolEscape);
    this.readingArea.addEventListener("pointerdown", event => this.placeNote(event));
    this.readingArea.addEventListener("mouseup", event => {
      if (event.button === 0 && this.annotationTool !== "note") window.setTimeout(() => this.finishTextSelection(), 0);
    });
    this.readingArea.addEventListener("keyup", () => this.captureSelection());
    this.readingArea.addEventListener("scroll", () => {
      this.hideSelectionPopup();
      this.scheduleReadingSave();
    });
    this.root.addEventListener("pointerdown", event => {
      const target = event.target as HTMLElement;
      if (!target.closest(".reader-appearance-popup, [data-action=text-options]")) this.closeAppearancePopup();
      if (target.closest(".reader-reflow") && !target.closest(".reader-selection-popup")) this.hideSelectionPopup();
    });
    this.root.addEventListener("keydown", event => {
      if (event.altKey && event.key.toLowerCase() === "r") {
        event.preventDefault(); void this.toggleReadingMode();
      }
      if (event.key === "Escape") {
        this.closeAppearancePopup(); this.hideSelectionPopup();
        this.noteComposer.close();
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault(); this.selectPane("search"); this.setSidebarOpen(true); this.searchInput.focus();
      }
    });
  }

  private readonly handleToolEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || this.destroyed) return;
    this.noteComposer.close();
    this.setAnnotationTool("pointer");
  };

  private readonly handleWheelZoom = (event: WheelEvent) => {
    if (!event.ctrlKey || event.deltaY === 0) return;

    event.preventDefault();
    event.stopPropagation();

    if (this.readingEnabled) {
      this.changeReadingAppearance({ fontSize: this.readingAppearance.fontSize + (event.deltaY < 0 ? 1 : -1) });
      return;
    }

    const factor = event.deltaY < 0 ? 1.1 : 1 / 1.1;
    this.wheelZoomFactor = Math.min(Math.max(this.wheelZoomFactor * factor, 0.2 / this.zoom), 6 / this.zoom);
    const areaRect = this.pageArea.getBoundingClientRect();
    const localX = event.clientX - areaRect.left;
    const localY = event.clientY - areaRect.top;
    const overPageArea = localX >= 0 && localY >= 0
      && localX <= areaRect.width
      && localY <= areaRect.height;
    this.wheelZoomAnchor = overPageArea ? { x: localX, y: localY } : null;

    if (!this.wheelZoomRunning) void this.applyWheelZoom();
  };

  private async applyWheelZoom() {
    if (this.wheelZoomRunning) return;
    this.wheelZoomRunning = true;

    try {
      while (this.wheelZoomFactor !== 1 && !this.destroyed) {
        const factor = this.wheelZoomFactor;
        const anchor = this.wheelZoomAnchor;
        this.wheelZoomFactor = 1;

        const oldScrollWidth = Math.max(1, this.pageArea.scrollWidth);
        const oldScrollHeight = Math.max(1, this.pageArea.scrollHeight);
        const oldRatioX = this.pageArea.scrollLeft / oldScrollWidth;
        const oldRatioY = this.pageArea.scrollTop / oldScrollHeight;
        const pointer = anchor ?? {
          x: this.pageArea.clientWidth / 2,
          y: this.pageArea.clientHeight / 2,
        };

        await this.setZoom(this.zoom * factor);
        if (this.destroyed) return;

        const newScrollWidth = Math.max(1, this.pageArea.scrollWidth);
        const newScrollHeight = Math.max(1, this.pageArea.scrollHeight);
        const focusRatioX = anchor
          ? (oldRatioX * oldScrollWidth + pointer.x) / oldScrollWidth
          : oldRatioX;
        const focusRatioY = anchor
          ? (oldRatioY * oldScrollHeight + pointer.y) / oldScrollHeight
          : oldRatioY;
        this.pageArea.scrollLeft = focusRatioX * newScrollWidth - pointer.x;
        this.pageArea.scrollTop = focusRatioY * newScrollHeight - pointer.y;
      }
    } finally {
      this.wheelZoomRunning = false;
    }
  }

  private metadataRow(label: string, value?: string | number | null) {
    if (value === undefined || value === null || value === "") return "";
    return `<div><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`;
  }

  private authorsText() {
    return (this.article?.authors ?? [])
      .map((author) => author.name || [author.last_name, author.fore_name].filter(Boolean).join(", "))
      .filter(Boolean)
      .join("; ");
  }

  private identifier(kind: string) {
    return this.article?.identifiers?.find((item) => item.kind.toLowerCase() === kind)?.value ?? null;
  }

  private externalHref() {
    const doi = this.identifier("doi");
    if (doi) return `https://doi.org/${encodeURIComponent(doi.replace(/^doi:/i, ""))}`;
    return this.identifier("url") ?? null;
  }

  private renderSidebarTags() {
    const tags = this.tagsPane.querySelector("[data-output=sidebar-tags]");
    if (tags) {
      const keywords = this.article?.keywords ?? [];
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

  private async load(bytes: Uint8Array) {
    this.setStatus("Parsing PDF…");
    this.document = await pdfjs.getDocument({
      data: bytes,
      useSystemFonts: true,
    }).promise;
    if (this.destroyed) return;

    this.pageCount = this.document.numPages;
    this.pageCountElement.textContent = `/ ${this.pageCount.toLocaleString()}`;
    this.pageNumberInput.max = String(this.pageCount);
    this.currentPage = 1;

    this.pageObserver = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        void this.renderPage(Number((entry.target as HTMLElement).dataset.page));
      }
      this.updateCurrentFromScroll();
    }, { root: this.pageArea, rootMargin: "600px 300px" });
    this.resizeObserver = new ResizeObserver(() => {
      if (this.destroyed || this.readingEnabled || this.fitMode === "custom") return;
      if (this.relayoutTimer !== null) window.clearTimeout(this.relayoutTimer);
      this.relayoutTimer = window.setTimeout(() => {
        this.relayoutTimer = null;
        void this.refit();
      }, 120);
    });
    this.resizeObserver.observe(this.pageArea);

    this.createPageViews();
    await Promise.all([this.loadThumbnails(), this.loadOutline(), this.loadAnnotations()]);
    await this.refit();
    try {
      const saved = JSON.parse(localStorage.getItem(`autonomics.pdf.readingLocation.${readerLocationId(this.articleId, this.attachmentKey)}`) ?? "null");
      if (!this.initialSearch && saved?.enabled && !this.destroyed) {
        if (Number.isInteger(saved.page)) this.setCurrentPage(Math.max(1, Math.min(this.pageCount, saved.page)), true);
        void this.toggleReadingMode();
      }
    } catch { /* Invalid saved positions do not interrupt PDF loading. */ }
  }

  private createPageViews() {
    this.pages.replaceChildren();
    for (let page = 1; page <= this.pageCount; page += 1) {
      const wrapper = document.createElement("div");
      wrapper.className = "reader-page-wrapper";
      wrapper.dataset.page = String(page);
      const canvas = document.createElement("canvas");
      const text = document.createElement("div");
      text.className = "reader-text-layer";
      wrapper.append(canvas, text);
      this.pages.append(wrapper);
      this.pageViews.set(page, { wrapper, canvas, text, viewport: null, task: null, textTask: null, rendered: false, rendering: false, generation: 0 });
      this.pageObserver?.observe(wrapper);
    }
  }

  private async loadThumbnails() {
    this.thumbnailsPane.replaceChildren();
    for (let page = 1; page <= this.pageCount; page += 1) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "reader-thumbnail";
      button.dataset.page = String(page);
      button.innerHTML = `
        <span class="reader-thumbnail-canvas"><span>Loading…</span></span>
        <span>${page.toLocaleString()}</span>`;
      button.addEventListener("click", () => void this.gotoPage(page));
      this.thumbnailsPane.append(button);
    }

    const observer = new IntersectionObserver((entries, thumbnailObserver) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const button = entry.target as HTMLButtonElement;
        thumbnailObserver.unobserve(button);
        void this.renderThumbnail(button, Number(button.dataset.page));
      }
    }, { root: this.thumbnailsPane, rootMargin: "300px" });
    this.thumbnailsPane
      .querySelectorAll<HTMLButtonElement>(".reader-thumbnail")
      .forEach((button) => observer.observe(button));
  }

  private async renderThumbnail(button: HTMLButtonElement, page: number) {
    if (this.destroyed || !this.document || this.thumbnails.has(page)) return;
    this.thumbnails.add(page);
    try {
      const proxy = await this.document.getPage(page);
      const base = proxy.getViewport({ scale: 1, rotation: 0 });
      const viewport = proxy.getViewport({ scale: 226 / base.width });
      const canvas = document.createElement("canvas");
      canvas.width = Math.ceil(viewport.width);
      canvas.height = Math.ceil(viewport.height);
      await proxy.render({
        canvasContext: canvas.getContext("2d")!,
        viewport,
      }).promise;
      const holder = button.querySelector(".reader-thumbnail-canvas");
      if (holder && !this.destroyed) holder.replaceChildren(canvas);
      else canvas.remove();
    } catch {
      this.thumbnails.delete(page);
    }
  }

  private async loadOutline() {
    if (!this.document) return;
    const raw = await this.document.getOutline();
    if (!raw?.length) {
      this.outlinePane.innerHTML = `<div class="reader-empty">No outline</div>`;
      return;
    }

    const entries: OutlineEntry[] = [];
    const walk = async (items: any[], depth: number) => {
      for (const item of items) {
        entries.push({
          title: item.title || "Untitled",
          page: await this.destinationPage(item.dest),
          depth,
        });
        if (item.items?.length) await walk(item.items, depth + 1);
      }
    };
    await walk(raw, 0);

    this.outlinePane.replaceChildren();
    for (const entry of entries) {
      const button = document.createElement("button");
      button.type = "button";
      button.style.paddingLeft = `${12 + entry.depth * 14}px`;
      button.textContent = entry.title;
      button.addEventListener("click", () => void this.gotoPage(entry.page));
      this.outlinePane.append(button);
    }
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
    for (const [page, view] of this.pageViews) this.renderPageAnnotations(page, view);
    this.readingView?.setAnnotations(this.annotations);
    this.annotationInspector.setAnnotations(this.annotations);
  }

  private renderPageAnnotations(page: number, view: PageView) {
    view.wrapper.querySelectorAll(".reader-annotation,.reader-highlight-note-indicator").forEach((node) => node.remove());
    const viewport = view.viewport;
    if (!viewport) return;
    for (const annotation of this.annotations) {
      if (Number(annotation.page) !== page || !Array.isArray(annotation.rects)) continue;
      const isNote = annotation.kind === "note" || annotation.kind === "comment";
      if (!isNote && annotation.kind !== "highlight") continue;
      const boxes = (isNote ? annotation.rects.slice(0, 1) : annotation.rects)
        .map((rect: PdfAnnotationRect) => this.annotationDisplayBox(rect, viewport))
        .filter((box: ReturnType<ZoteroStylePdfReader["annotationDisplayBox"]>) => box !== null);
      for (const box of boxes) {
        const node = document.createElement("div");
        node.className = "reader-annotation";
        node.dataset.annotationId = annotation.id;
        node.style.left = `${box.left * 100}%`;
        node.style.top = `${box.top * 100}%`;
        node.style.width = `${box.width * 100}%`;
        node.style.height = `${box.height * 100}%`;
        node.style.background = annotation.color ?? "rgba(255, 214, 0, .38)";
        if (isNote) {
          node.classList.add("reader-note-marker");
          node.style.left = `${(box.left + box.width / 2) * 100}%`;
          node.style.top = `${(box.top + box.height / 2) * 100}%`;
          node.style.width = "22px"; node.style.height = "22px";
          node.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${TOOLBAR_ICONS.note}</svg>`;
          node.setAttribute("aria-label", "Note");
        }
        node.title = isNote ? annotation.content : "Click to select annotation";
        view.wrapper.append(node);
      }
      if (!isNote && boxes.length) {
        const indicator = highlightNoteIndicator(annotation, id => this.openHighlightNote(id));
        if (indicator) {
          const first = [...boxes].sort((a, b) => a.top - b.top || a.left - b.left)[0];
          indicator.style.left = `${Math.min(viewport.width - 12, (first.left + first.width) * viewport.width + 17)}px`;
          indicator.style.top = `${Math.max(12, first.top * viewport.height + first.height * viewport.height / 2)}px`;
          view.wrapper.append(indicator);
        }
      }
    }

  }

  private openHighlightNote(id: string) {
    this.setAnnotationTool("pointer");
    this.annotationInspector.select(id);
  }

  private annotationDisplayBox(
    rect: PdfAnnotationRect,
    viewport: PageViewport,
  ): { left: number; top: number; width: number; height: number } | null {
    if (rect.space !== "pdf") {
      return {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      };
    }

    const [pageX0, pageY0, pageX1, pageY1] = viewport.viewBox;
    const pageWidth = Math.abs(pageX1 - pageX0);
    const pageHeight = Math.abs(pageY1 - pageY0);
    if (!pageWidth || !pageHeight) return null;

    const pdfLeft = pageX0 + rect.left * pageWidth;
    const pdfRight = pageX0 + (rect.left + rect.width) * pageWidth;
    const pdfBottom = pageY0 + rect.top * pageHeight;
    const pdfTop = pageY0 + (rect.top + rect.height) * pageHeight;
    const [left, top, right, bottom] = viewport.convertToViewportRectangle([
      pdfLeft,
      pdfBottom,
      pdfRight,
      pdfTop,
    ]);
    return {
      left: Math.min(left, right) / viewport.width,
      top: Math.min(top, bottom) / viewport.height,
      width: Math.abs(right - left) / viewport.width,
      height: Math.abs(bottom - top) / viewport.height,
    };
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
        <strong>Page ${Number(annotation.page ?? 1).toLocaleString()}</strong>
        <small>${escapeHtml(annotation.content.slice(0, 180))}</small>
        ${annotation.comment ? `<small class="reader-annotation-list-comment">${escapeHtml(annotation.comment)}</small>` : ""}
        ${annotation.tags?.length ? `<small>${annotation.tags.map((tag: string) => escapeHtml(tag)).join(" · ")}</small>` : ""}`;
      button.addEventListener("click", async () => {
        if (this.readingEnabled) this.readingView?.navigateAnnotation(annotation);
        else { await this.gotoPage(Number(annotation.page ?? 1)); await this.renderPage(Number(annotation.page ?? 1)); }
        if (!this.destroyed) this.annotationInspector.select(annotation.id);
      });
      this.annotationPane.append(button);
    }
  }

  private captureSelection() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) {
      this.hideSelectionPopup();
      return;
    }
    const range = selection.getRangeAt(0);
    if (this.readingEnabled) {
      this.captureReadingSelection(range);
      return;
    }
    const element = range.commonAncestorContainer.nodeType === Node.TEXT_NODE
      ? range.commonAncestorContainer.parentElement
      : range.commonAncestorContainer as HTMLElement;
    const wrapper = element?.closest<HTMLElement>(".reader-page-wrapper");
    if (!wrapper || !this.pageArea.contains(wrapper)) {
      this.hideSelectionPopup();
      return;
    }
    const layer = element?.closest<HTMLElement>(".reader-text-layer");
    const view = this.pageViews.get(Number(wrapper.dataset.page));
    if (!layer || !view?.viewport) {
      this.hideSelectionPopup();
      return;
    }
    // The text layer is rotated in CSS; use the displayed page as the origin.
    const layerRect = wrapper.getBoundingClientRect();
    const viewport = view.viewport;
    const scaleX = viewport.width / layerRect.width;
    const scaleY = viewport.height / layerRect.height;
    const [pageX0, , pageX1, pageY1] = viewport.viewBox;
    const pageWidth = Math.abs(pageX1 - pageX0);
    const pageHeight = Math.abs(pageY1 - viewport.viewBox[1]);
    const { rects: clientRects, text: visibleText } = this.visibleSelectionGeometry(range, layer);
    const rects = clientRects
      .map((rect) => {
        const localLeft = (rect.left - layerRect.left) * scaleX;
        const localTop = (rect.top - layerRect.top) * scaleY;
        const [pdfLeft, pdfBottom] = viewport.convertToPdfPoint(
          localLeft,
          localTop + rect.height * scaleY,
        );
        const [pdfRight, pdfTop] = viewport.convertToPdfPoint(
          localLeft + rect.width * scaleX,
          localTop,
        );
        return {
          left: Math.max(0, (Math.min(pdfLeft, pdfRight) - pageX0) / pageWidth),
          top: Math.max(0, (Math.min(pdfBottom, pdfTop) - viewport.viewBox[1]) / pageHeight),
          width: Math.min(1, Math.abs(pdfRight - pdfLeft) / pageWidth),
          height: Math.min(1, Math.abs(pdfTop - pdfBottom) / pageHeight),
          space: "pdf" as const,
        };
      })
      .filter((rect) => rect.width > 0 && rect.height > 0);
    if (!rects.length || !visibleText) {
      this.hideSelectionPopup();
      return;
    }
    const last = clientRects[clientRects.length - 1];
    this.anchor = {
      page: Number(wrapper.dataset.page),
      text: visibleText,
      rects,
      x: last.left,
      y: last.top + last.height + 8,
    };
    this.selectionPopup.style.left = `${Math.min(this.anchor.x, window.innerWidth - 270)}px`;
    this.selectionPopup.style.top = `${Math.min(this.anchor.y, window.innerHeight - 60)}px`;
    this.selectionPopup.hidden = false;
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

  private hideSelectionPopup() {
    this.selectionPopup.hidden = true;
    this.anchor = null;
  }

  private visibleSelectionGeometry(range: Range, layer: HTMLElement): {
    rects: Array<{ left: number; top: number; width: number; height: number }>;
    text: string;
  } {
    const rects: Array<{ left: number; top: number; width: number; height: number }> = [];
    const textParts: string[] = [];
    const walker = document.createTreeWalker(layer, NodeFilter.SHOW_TEXT);

    let node = walker.nextNode() as Text | null;
    while (node) {
      if (!range.intersectsNode(node)) {
        node = walker.nextNode() as Text | null;
        continue;
      }

      const length = node.data.length;
      let start = 0;
      let end = length;
      if (node === range.startContainer) start = range.startOffset;
      if (node === range.endContainer) end = range.endOffset;

      start = Math.max(0, Math.min(start, length));
      end = Math.max(start, Math.min(end, length));

      let index = start;
      while (index < end) {
        while (index < end && /\s/.test(node.data[index])) index += 1;
        if (index >= end) break;

        const visibleStart = index;
        while (index < end && !/\s/.test(node.data[index])) index += 1;
        const visibleEnd = index;

        const segment = document.createRange();
        segment.setStart(node, visibleStart);
        segment.setEnd(node, visibleEnd);
        for (const rect of segment.getClientRects()) {
          if (rect.width > 0.5 && rect.height > 0.5) {
            rects.push({
              left: rect.left,
              top: rect.top,
              width: rect.width,
              height: rect.height,
            });
          }
        }
        textParts.push(node.data.slice(visibleStart, visibleEnd));
      }

      node = walker.nextNode() as Text | null;
    }

    const sorted = [...rects].sort((left, right) => {
      const leftCenter = left.top + left.height / 2;
      const rightCenter = right.top + right.height / 2;
      return leftCenter - rightCenter || left.left - right.left;
    });
    const merged: Array<{ left: number; top: number; width: number; height: number }> = [];
    for (const rect of sorted) {
      const previous = merged[merged.length - 1];
      const previousCenter = previous ? previous.top + previous.height / 2 : 0;
      const currentCenter = rect.top + rect.height / 2;
      const sameLine = previous && Math.abs(currentCenter - previousCenter) <= Math.max(previous.height, rect.height) * .45;
      const gap = previous ? rect.left - (previous.left + previous.width) : Number.POSITIVE_INFINITY;
      if (previous && sameLine && gap <= rect.height * 2.5) {
        const left = Math.min(previous.left, rect.left);
        const top = Math.min(previous.top, rect.top);
        const right = Math.max(previous.left + previous.width, rect.left + rect.width);
        const bottom = Math.max(previous.top + previous.height, rect.top + rect.height);
        merged[merged.length - 1] = {
          left,
          top,
          width: right - left,
          height: bottom - top,
        };
      } else {
        merged.push({ ...rect });
      }
    }

    return {
      rects: merged,
      text: textParts.join(" ").replace(/\s+/g, " ").trim(),
    };
  }

  private setAnnotationTool(tool: AnnotationTool) {
    this.annotationTool = tool;
    this.root.dataset.annotationTool = tool;
    for (const name of ["highlight", "note"] as const) {
      this.root.querySelector(`[data-action="tool-${name}"]`)?.setAttribute("aria-pressed", String(tool === name));
    }
    this.annotationInspector.close();
    if (tool === "highlight") this.toast.show("Drag across text to highlight it", "info");
    if (tool === "note") this.toast.show("Click on the page to place a note", "info");
  }

  private finishTextSelection() {
    if (this.destroyed) return;
    this.captureSelection();
    if (this.annotationTool === "highlight" && this.anchor) {
      this.selectionPopup.hidden = true;
      void this.addHighlight("#ffd400");
    }
  }

  private async saveAnnotation(input: { content: string; kind: string; page: number; rects: PdfAnnotationRect[]; color?: string }): Promise<ReaderAnnotation | null> {
    try {
      const response = await gatewayRequest("POST", annotationPath(this.articleId, this.attachmentKey), input);
      if (this.destroyed) return null;
      const annotation = response.annotation as ReaderAnnotation;
      // Use the write response immediately instead of waiting for a second
      // request. A transient list failure must not hide a successful write.
      this.annotations.push(annotation);
      this.renderAnnotations(); this.renderAnnotationList(); this.renderSidebarTags();
      return annotation;
    } catch {
      if (!this.destroyed) this.toast.show("Could not save annotation. Please try again.", "error");
      return null;
    }
  }

  private async addHighlight(color: string) {
    if (!this.anchor || this.savingHighlights.has(this.anchor)) return;
    const anchor = this.anchor;
    color = ({ yellow: "#ffd400", green: "#5fb236", red: "#ff6666" } as Record<string, string>)[color] ?? color;
    this.savingHighlights.add(anchor);
    try {
      for (const target of [{ page: anchor.page, rects: anchor.rects }, ...(anchor.extraPages ?? [])]) {
        const saved = await this.saveAnnotation({ content: anchor.text, kind: "highlight", page: target.page, color, rects: target.rects });
        if (!saved) {
          if (!this.destroyed && this.anchor === anchor) this.selectionPopup.hidden = false;
          return;
        }
      }
      // Do not clear a newer selection made while this write was in flight.
      if (this.anchor === anchor) {
        this.hideSelectionPopup(); window.getSelection()?.removeAllRanges();
      }
      if (!this.destroyed) this.toast.show("Highlight saved");
    } finally {
      this.savingHighlights.delete(anchor);
    }
  }

  private placeNote(event: PointerEvent) {
    if (this.annotationTool !== "note" || event.button !== 0) return;
    const element = event.target as HTMLElement;
    if (this.readingEnabled) {
      const range = document.caretRangeFromPoint?.(event.clientX, event.clientY);
      if (!range || !this.readingArea.contains(range.startContainer) || range.startContainer.nodeType !== Node.TEXT_NODE) {
        this.toast.show("Click on text to place a note in Reading Mode", "info"); return;
      }
      const node = range.startContainer as Text;
      const offset = Math.min(range.startOffset, node.length - 1);
      if (offset < 0) return;
      range.setStart(node, offset); range.setEnd(node, offset + 1);
      this.captureReadingSelection(range);
      if (!this.anchor) return;
      event.preventDefault();
      this.addNote(this.anchor, { x: event.clientX, y: event.clientY });
      return;
    }
    const wrapper = element.closest<HTMLElement>(".reader-page-wrapper");
    const view = wrapper && this.pageViews.get(Number(wrapper.dataset.page));
    if (!wrapper || !view?.viewport) return;
    event.preventDefault();
    const box = wrapper.getBoundingClientRect(), viewport = view.viewport;
    const [x, y] = viewport.convertToPdfPoint((event.clientX - box.left) * viewport.width / box.width, (event.clientY - box.top) * viewport.height / box.height);
    const [x0, y0, x1, y1] = viewport.viewBox;
    const width = Math.min(1, 22 / (x1 - x0)), height = Math.min(1, 22 / (y1 - y0));
    this.addNote({
      page: Number(wrapper.dataset.page), text: "", x: event.clientX, y: event.clientY,
      rects: [{ left: Math.max(0, Math.min(1 - width, (x - x0) / (x1 - x0) - width / 2)), top: Math.max(0, Math.min(1 - height, (y - y0) / (y1 - y0) - height / 2)), width, height, space: "pdf" }],
    });
  }

  private addNote(target = this.anchor, point?: { x: number; y: number }) {
    if (!target) {
      this.setAnnotationTool("note");
      return;
    }
    this.setAnnotationTool("pointer");
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    this.noteComposer.open(point ?? target, async content => {
      const annotation = await this.saveAnnotation({ content, kind: "note", page: target.page, rects: target.rects, color: "#ffd400" });
      if (!annotation) return false;
      this.toast.show("Note saved");
      this.annotationInspector.select(annotation.id);
      return true;
    });
  }

  private async updateAnnotation(id: string, patch: AnnotationPatch): Promise<ReaderAnnotation> {
    const response = await gatewayRequest("PATCH", `/bib/annotations/${encodeURIComponent(id)}`, patch);
    const annotation = response.annotation as ReaderAnnotation;
    if (!this.destroyed) {
      this.annotations = this.annotations.map(value => value.id === id ? annotation : value);
      this.renderAnnotations(); this.renderAnnotationList();
    }
    return annotation;
  }

  private async deleteAnnotation(id: string) {
    const confirmed = await this.alertDialog.open({
      title: "Delete this annotation?",
      description: "The annotation will be permanently removed from the document.",
      actionLabel: "Delete",
      destructive: true,
    });
    if (!confirmed) return;
    await gatewayRequest("DELETE", `/bib/annotations/${encodeURIComponent(id)}`);
    await this.loadAnnotations();
    this.renderSidebarTags();
  }

  private async pageText(page: number) {
    const cached = this.textCache.get(page);
    if (cached !== undefined) return cached;
    const proxy = await this.document?.getPage(page);
    if (!proxy) return "";
    const content = await proxy.getTextContent();
    const text = content.items.map((item: any) => item.str ?? "").join(" ").toLowerCase();
    this.textCache.set(page, text);
    return text;
  }

  private searchSequence = 0;
  private async revealSearch(target: ReaderSearchTarget) {
    if (target.annotationId) {
      const annotation = this.annotations.find(note => note.id === target.annotationId);
      if (!annotation) throw new Error("The annotation changed. Refresh search results.");
      const page = target.page ?? annotation.page;
      if (page) { if (page < 1 || page > this.pageCount) throw new Error("The evidence page is no longer available. Refresh search results."); await this.gotoPage(page); await this.renderPage(page); }
      this.selectPane("annotations"); this.setSidebarOpen(true); this.annotationInspector.select(target.annotationId); return;
    }
    if (target.query) {
      this.searchInput.value = target.query;this.selectPane("search");this.setSidebarOpen(true);this.searchInput.focus();
      await this.searchPdf(target.query, true);
      if (this.searchResults[0] && !this.destroyed) await this.gotoPage(this.searchResults[0].page);
    }
  }
  private async searchPdf(term: string, wordBoundary = false) {
    const sequence = ++this.searchSequence;
    if (!this.document || !term) { this.searchResults = []; this.renderSearchResults(); return; }
    this.setStatus(`Searching “${term}”…`);
    const results: Array<{page:number;count:number}> = [];
    try {
      for (let page = 1; page <= this.pageCount; page++) {
        const text = await this.pageText(page);
        if (this.destroyed || sequence !== this.searchSequence) return;
        const count = findTextRanges(text, term, wordBoundary).length;
        if (count) results.push({page,count});
      }
    } catch (error) { if (!this.destroyed && sequence === this.searchSequence) this.setStatus(`Search unavailable: ${String(error)}`); return; }
    if (this.destroyed || sequence !== this.searchSequence) return;
    this.searchResults = results; this.renderSearchResults(); this.selectPane("search");
    this.setStatus(results.length ? `${results.length.toLocaleString()} pages match` : "Indexed text could not be located in this file. Refresh the search or try another match.");
  }

  private renderSearchResults() {
    const output = this.searchPane.querySelector("[data-output=search-results]")!;
    output.replaceChildren();
    if (!this.searchResults.length) {
      output.innerHTML = `<div class="reader-empty">No matches</div>`;
      return;
    }
    for (const result of this.searchResults) {
      const button = document.createElement("button");
      button.type = "button";
      button.innerHTML = `<strong>Page ${result.page.toLocaleString()}</strong><small>${result.count.toLocaleString()} matches</small>`;
      button.addEventListener("click", () => void this.gotoPage(result.page));
      output.append(button);
    }
  }

  private selectPane(name: string) {
    const pane = name as ReaderPane;
    this.activePane = pane;
    this.root.querySelectorAll<HTMLButtonElement>(".reader-rail-button").forEach((button) => {
      button.classList.toggle("active", button.dataset.pane === name);
    });
    const title = this.root.querySelector("[data-output=sidebar-title]");
    if (title) title.textContent = READER_RAIL_LABELS[pane];
    this.outlinePane.hidden = name !== "outline";
    this.thumbnailsPane.hidden = name !== "thumbnails";
    this.annotationPane.hidden = name !== "annotations";
    this.infoPane.hidden = name !== "info";
    this.tagsPane.hidden = name !== "tags";
    this.searchPane.hidden = name !== "search";
  }

  private setSidebarOpen(open: boolean) {
    this.sidebarOpen = open;
    this.root.classList.toggle("sidebar-closed", !open);
    window.localStorage.setItem(READER_SIDEBAR_OPEN_KEY, open ? "1" : "0");
    window.setTimeout(() => void this.refit(), 170);
  }

  private async destinationPage(destination: any): Promise<number> {
    if (!this.document) return 1;
    try {
      const explicit = typeof destination === "string"
        ? await this.document.getDestination(destination)
        : destination;
      const reference = explicit?.[0];
      if (!reference) return 1;
      return (await this.document.getPageIndex(reference)) + 1;
    } catch {
      return 1;
    }
  }

  private async gotoPage(page: number) {
    const target = Math.min(Math.max(1, Math.floor(page || 1)), this.pageCount);
    if (this.readingEnabled) {
      this.readingView?.gotoPage(target);
      this.setCurrentPage(target, true);
      return;
    }
    const wrapper = this.pageViews.get(target)?.wrapper;
    if (wrapper) {
      this.pageArea.scrollTo({ top: Math.max(0, wrapper.offsetTop - 12), behavior: "smooth" });
    }
    this.setCurrentPage(target, true);
    void this.renderPage(target);
  }

  private setCurrentPage(page: number, updateInput = true) {
    this.currentPage = page;
    if (updateInput) this.pageNumberInput.value = String(page);
    this.thumbnailsPane
      .querySelectorAll<HTMLButtonElement>(".reader-thumbnail")
      .forEach((button) => {
        button.classList.toggle("active", Number(button.dataset.page) === page);
        if (Number(button.dataset.page) === page) {
          button.scrollIntoView({ block: "nearest", behavior: "smooth" });
        }
      });
  }

  private updateCurrentFromScroll() {
    if (!this.pageCount) return;
    const center = this.pageArea.scrollTop + this.pageArea.clientHeight / 2;
    let current = 1;
    for (let page = 1; page <= this.pageCount; page += 1) {
      const view = this.pageViews.get(page);
      if (!view) continue;
      if (view.wrapper.offsetTop <= center) current = page;
      if (view.wrapper.offsetTop + view.wrapper.offsetHeight > center) break;
    }
    if (current !== this.currentPage) this.setCurrentPage(current, true);
  }

  private async setZoom(zoom: number) {
    if (this.readingEnabled) {
      this.changeReadingAppearance({ fontSize: this.readingAppearance.fontSize + (zoom > this.zoom ? 1 : -1) });
      return;
    }
    this.fitMode = "custom";
    this.updateFitButtons();
    this.zoom = Math.min(Math.max(zoom, 0.2), 6);
    this.zoomLabel.textContent = `${Math.round(this.zoom * 100)}%`;
    await this.relayout();
  }

  private updateFitButtons() {
    for (const mode of ["width", "page"] as const) {
      this.root.querySelector(`[data-action="fit-${mode}"]`)
        ?.setAttribute("aria-pressed", String(this.fitMode === mode));
    }
  }

  private async refit() {
    this.updateFitButtons();
    if (!this.document || this.destroyed || this.readingEnabled) return;
    if (this.fitMode !== "custom") {
      const proxy = await this.document.getPage(1);
      const base = proxy.getViewport({ scale: 1, rotation: (proxy.rotate + this.rotation) % 360 });
      const availableWidth = Math.max(180, this.pageArea.clientWidth - 44);
      const availableHeight = Math.max(180, this.pageArea.clientHeight - 72);
      this.zoom = this.fitMode === "page"
        ? Math.min(availableWidth / base.width, availableHeight / base.height)
        : availableWidth / base.width;
      this.zoomLabel.textContent = `${Math.round(this.zoom * 100)}%`;
    }
    await this.relayout();
  }

  private async relayout() {
    if (!this.document || this.destroyed) return;
    const layoutGeneration = ++this.layoutGeneration;
    this.textSelection.clear();
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    this.setStatus("Rendering…");
    for (let page = 1; page <= this.pageCount; page += 1) {
      const proxy = await this.document.getPage(page);
      if (this.destroyed || layoutGeneration !== this.layoutGeneration) return;
      const base = proxy.getViewport({ scale: 1, rotation: (proxy.rotate + this.rotation) % 360 });
      this.pageInfos.set(page, { width: base.width, height: base.height });
    }

    for (const [page, view] of this.pageViews) {
      const info = this.pageInfos.get(page);
      if (!info) continue;
      const width = info.width * this.zoom;
      const height = info.height * this.zoom;
      view.wrapper.style.width = `${width}px`;
      view.wrapper.style.height = `${height}px`;
      view.canvas.style.width = `${width}px`;
      view.canvas.style.height = `${height}px`;
      view.text.style.width = `${width}px`;
      view.text.style.height = `${height}px`;
      this.cancelRender(view);
      view.viewport = null;
      view.wrapper.querySelectorAll(".reader-annotation,.reader-highlight-note-indicator").forEach((node) => node.remove());
      view.text.replaceChildren();
      view.canvas.width = 0;
      view.canvas.height = 0;
      view.wrapper.classList.add("rendering");
    }

    this.renderVisiblePages();
  }

  private renderVisiblePages() {
    if (this.readingEnabled) return;
    const area = this.pageArea.getBoundingClientRect();
    for (const [page, view] of this.pageViews) {
      const rect = view.wrapper.getBoundingClientRect();
      const visible = rect.bottom >= area.top - 800 && rect.top <= area.bottom + 800;
      if (visible) void this.renderPage(page);
    }
  }

  private cancelRender(view: PageView) {
    view.generation += 1;
    view.task?.cancel();
    view.textTask?.cancel();
    view.textTask = null;
    view.rendered = false;
    view.task = null;
    view.rendering = false;
  }

  private alignTextWidths(
    textLayer: TextLayer,
    content: Awaited<ReturnType<PDFPageProxy["getTextContent"]>>,
    viewport: PageViewport,
  ) {
    // PDF.js 4.10 can reuse a cached canvas font after resizing its measurement
    // canvas (which resets that font). At a measured size of 30px this produces
    // a scaleX about three times too large. Measure the actual DOM advances and
    // enforce the PDF's widths instead, keeping PDF.js's rotation/min-font scale.
    const items = content.items.filter((item) => "str" in item);
    const measurements: Array<{ div: HTMLElement; transform: string; width: number }> = [];
    textLayer.textDivs.forEach((div, index) => {
      const transform = div.style.transform;
      const item = items[index];
      if (!item || !/scaleX\(/.test(transform) || !div.isConnected) return;
      const vertical = content.styles[item.fontName]?.vertical;
      const width = (vertical ? item.height : item.width) * viewport.scale;
      if (!(width > 0)) return;
      measurements.push({ div, transform, width });
      div.style.transform = transform.replace(/scaleX\([^)]*\)|rotate\([^)]*\)/g, "");
    });
    // Batch writes and reads to avoid forcing layout once per text item.
    const widths = measurements.map(({ div }) => {
      const rect = div.getBoundingClientRect();
      return viewport.rotation % 180 ? rect.height : rect.width;
    });
    measurements.forEach(({ div, transform, width }, index) => {
      const measured = widths[index];
      div.style.transform = measured > 0
        ? transform.replace(/scaleX\([^)]*\)/, `scaleX(${width / measured})`)
        : transform;
    });
  }

  private async renderPage(page: number) {
    const view = this.pageViews.get(page);
    const document = this.document;
    if (!document || this.destroyed || !view || view.rendering || view.rendered || !view.wrapper.style.width) return;
    const generation = view.generation;
    view.rendering = true;
    try {
      const proxy = await document.getPage(page);
      if (this.destroyed || view.generation !== generation) return;
      const viewport = proxy.getViewport({ scale: this.zoom, rotation: (proxy.rotate + this.rotation) % 360 });
      view.viewport = viewport;
      const outputScale = Math.min(window.devicePixelRatio || 1, 2);
      view.canvas.width = Math.floor(viewport.width * outputScale);
      view.canvas.height = Math.floor(viewport.height * outputScale);
      const context = view.canvas.getContext("2d")!;
      const scaleX = view.canvas.width / viewport.width;
      const scaleY = view.canvas.height / viewport.height;
      context.setTransform(scaleX, 0, 0, scaleY, 0, 0);
      context.clearRect(0, 0, viewport.width, viewport.height);

      const task = proxy.render({
        canvasContext: context,
        viewport,
      });
      view.task = task;
      await task.promise;
      if (this.destroyed || view.generation !== generation) return;

      view.text.replaceChildren();
      view.text.style.setProperty("--scale-factor", String(viewport.scale));
      const textContent = await proxy.getTextContent();
      if (this.destroyed || view.generation !== generation) return;
      // Match the language of PDF.js's measurement canvas. Inheriting the UI
      // language (zh-CN) can select a different generic font and stretch every line.
      view.text.lang = textContent.lang ?? "";
      const textLayer = new TextLayer({
        textContentSource: textContent,
        container: view.text,
        viewport,
      });
      view.textTask = textLayer;
      await textLayer.render();
      if (this.destroyed || view.generation !== generation) return;
      this.alignTextWidths(textLayer, textContent, viewport);
      this.textSelection.register(view.text);
      view.rendered = true;
      this.renderPageAnnotations(page, view);
      view.wrapper.classList.remove("rendering");
      if (page === this.currentPage) {
        this.setStatus(`${page.toLocaleString()} / ${this.pageCount.toLocaleString()}`);
      }
    } catch (error) {
      if (view.generation === generation && !this.destroyed &&
          !["RenderingCancelledException", "AbortException"].includes((error as any)?.name)) {
        this.setStatus(`Render failed: ${String(error)}`);
      }
    } finally {
      if (view.generation === generation) {
        view.rendering = false;
        view.task = null;
        view.textTask = null;
      }
    }
  }

  destroy() {
    if (this.destroyed) return;
    this.saveReadingLocation();
    this.destroyed = true;
    releaseReaderSession(this.sessionToken);
    this.toast.destroy();
    this.alertDialog.destroy();
    this.noteComposer.destroy();
    if (this.readingSaveTimer !== null) clearTimeout(this.readingSaveTimer);
    this.annotationInspector.destroy();
    this.readingView?.destroy();
    restoreReaderFocus(this.returnFocus);
    if (this.relayoutTimer !== null) window.clearTimeout(this.relayoutTimer);
    this.root.removeEventListener("wheel", this.handleWheelZoom);
    document.removeEventListener("keydown", this.handleToolEscape);
    this.textSelection.destroy();
    this.resizeObserver?.disconnect();
    this.pageObserver?.disconnect();
    for (const view of this.pageViews.values()) {
      this.cancelRender(view);
      view.text.replaceChildren();
    }
    this.document?.destroy().catch(() => undefined);
    this.root.remove();
  }
}
