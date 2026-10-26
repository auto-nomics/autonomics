import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy, PageViewport, RenderTask } from "pdfjs-dist";
import { TextLayer } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { fetchBibFulltextRaw, gatewayRequest } from "./gateway";
import { escapeHtml } from "./markdown";
import type { BibArticle } from "./types";
import "./pdf-reader.css";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

type OutlineEntry = {
  title: string;
  page: number;
  depth: number;
};

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
  rendering: boolean;
  generation: number;
};

type SelectionAnchor = {
  page: number;
  text: string;
  rects: Array<PdfAnnotationRect>;
  x: number;
  y: number;
};

type PdfAnnotationRect = {
  left: number;
  top: number;
  width: number;
  height: number;
  space?: "display" | "pdf";
};

type ReaderPane = "thumbnails" | "outline" | "annotations" | "info" | "tags" | "search";

const READER_SIDEBAR_WIDTH_KEY = "autonomics.pdf.sidebarWidth";
const READER_SIDEBAR_OPEN_KEY = "autonomics.pdf.sidebarOpen";

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

export type PdfReaderHandle = {
  close: () => void;
};

let activeReader: PdfReaderHandle | null = null;

export async function openPdfReader(options: {
  articleId: string;
  title: string;
  article?: BibArticle | null;
}): Promise<PdfReaderHandle> {
  activeReader?.close();
  const reader = await ZoteroStylePdfReader.open(
    options.articleId,
    options.title,
    options.article ?? null,
  );
  const handle: PdfReaderHandle = {
    close: () => reader.destroy(),
  };
  activeReader = handle;
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

  private root: HTMLDivElement;
  private pageArea: HTMLDivElement;
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

  private constructor(
    private readonly articleId: string,
    private readonly title: string,
    private readonly article: BibArticle | null,
  ) {
    this.root = document.createElement("div");
    this.root.className = "zotero-pdf-reader";
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
        <header class="reader-toolbar">
          <button type="button" data-action="sidebar" title="Toggle sidebar" aria-label="Toggle sidebar">☰</button>
          <div class="reader-tool-group">
            <button type="button" data-action="previous" title="Previous page">‹</button>
            <input data-input="page" type="number" min="1" value="1" aria-label="Page" />
            <span data-output="page-count">/ 0</span>
            <button type="button" data-action="next" title="Next page">›</button>
          </div>
          <div class="reader-tool-group">
            <button type="button" data-action="zoom-out" title="Zoom out">−</button>
            <span data-output="zoom">100%</span>
            <button type="button" data-action="zoom-in" title="Zoom in">+</button>
            <button type="button" data-action="fit-width">Fit width</button>
            <button type="button" data-action="fit-page">Fit page</button>
          </div>
          <div class="reader-tool-center">
            <button type="button" data-action="tool-highlight" title="Highlight selection">A</button>
            <button type="button" data-action="tool-note" title="Add note">T</button>
            <button type="button" data-action="copy-selection-toolbar" title="Copy selection">⧉</button>
            <button type="button" data-action="tool-clear" title="Clear selection">⌫</button>
          </div>
          <div class="reader-tool-right">
            <button type="button" data-action="text-options" title="Text options">Aa</button>
            <button type="button" data-action="open-search" title="Search PDF">🔍</button>
          </div>
          <button type="button" data-action="close" title="Close">✕</button>
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

    this.pageArea = this.select(".reader-page-area");
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
  ): Promise<ZoteroStylePdfReader> {
    const reader = new ZoteroStylePdfReader(articleId, title, article);
    try {
      const raw = await fetchBibFulltextRaw(articleId);
      if (reader.destroyed) return reader;
      await reader.load(raw.bytes);
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
    action("close-sidebar", () => this.setSidebarOpen(false));
    action("tool-highlight", () => {
      if (this.anchor) void this.addHighlight("yellow");
      else this.setStatus("Select text first");
    });
    action("tool-note", () => this.addNote());
    action("copy-selection-toolbar", () => {
      if (this.anchor) void navigator.clipboard.writeText(this.anchor.text);
      else this.setStatus("Select text first");
    });
    action("tool-clear", () => {
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
    action("add-note", () => void this.addNote());
    action("copy-selection", () => {
      if (this.anchor) void navigator.clipboard.writeText(this.anchor.text);
      this.hideSelectionPopup();
    });

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
    this.pageArea.addEventListener("mouseup", () => window.setTimeout(() => this.captureSelection(), 0));
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
      if (this.destroyed || this.fitMode === "custom") return;
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
      this.pageViews.set(page, { wrapper, canvas, text, viewport: null, task: null, rendering: false, generation: 0 });
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
        `/bib/articles/${encodeURIComponent(this.articleId)}/annotations`,
      );
      this.annotations = response.annotations ?? [];
      this.renderAnnotations();
      this.renderAnnotationList();
    } catch (error) {
      this.setStatus(`Annotations unavailable: ${String(error)}`);
    }
  }

  private async renderAnnotations() {
    for (const view of this.pageViews.values()) {
      view.wrapper.querySelectorAll(".reader-annotation").forEach((node) => node.remove());
    }
    for (const annotation of this.annotations) {
      const view = this.pageViews.get(Number(annotation.page));
      if (!view || annotation.kind !== "highlight" || !Array.isArray(annotation.rects)) continue;
      const proxy = await this.document?.getPage(Number(annotation.page));
      if (!proxy) continue;
      const viewport = proxy.getViewport({ scale: this.zoom, rotation: this.rotation });
      for (const rect of annotation.rects) {
        const box = this.annotationDisplayBox(rect, viewport);
        if (!box) continue;
        const node = document.createElement("div");
        node.className = "reader-annotation";
        node.dataset.annotationId = annotation.id;
        node.style.left = `${box.left * 100}%`;
        node.style.top = `${box.top * 100}%`;
        node.style.width = `${box.width * 100}%`;
        node.style.height = `${box.height * 100}%`;
        node.style.background = annotation.color ?? "rgba(255, 214, 0, .38)";
        node.title = "Click to delete annotation";
        node.addEventListener("click", () => void this.deleteAnnotation(annotation.id));
        view.wrapper.append(node);
      }
    }

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
      button.innerHTML = `
        <strong>Page ${Number(annotation.page ?? 1).toLocaleString()}</strong>
        <small>${escapeHtml(annotation.content.slice(0, 180))}</small>`;
      button.addEventListener("click", () => void this.gotoPage(Number(annotation.page ?? 1)));
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
    const layerRect = layer.getBoundingClientRect();
    const viewport = view.viewport;
    const [pageX0, , pageX1, pageY1] = viewport.viewBox;
    const pageWidth = Math.abs(pageX1 - pageX0);
    const pageHeight = Math.abs(pageY1 - viewport.viewBox[1]);
    const rects = Array.from(range.getClientRects())
      .filter((rect) => rect.width > 1 && rect.height > 1)
      .map((rect) => {
        const localLeft = rect.left - layerRect.left;
        const localTop = rect.top - layerRect.top;
        const [pdfLeft, pdfBottom] = viewport.convertToPdfPoint(
          localLeft,
          localTop + rect.height,
        );
        const [pdfRight, pdfTop] = viewport.convertToPdfPoint(
          localLeft + rect.width,
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
    if (!rects.length || !selection.toString().trim()) {
      this.hideSelectionPopup();
      return;
    }
    const last = range.getClientRects()[range.getClientRects().length - 1];
    this.anchor = {
      page: Number(wrapper.dataset.page),
      text: selection.toString(),
      rects,
      x: last.left,
      y: last.bottom + 8,
    };
    this.selectionPopup.style.left = `${Math.min(this.anchor.x, window.innerWidth - 270)}px`;
    this.selectionPopup.style.top = `${Math.min(this.anchor.y, window.innerHeight - 60)}px`;
    this.selectionPopup.hidden = false;
  }

  private hideSelectionPopup() {
    this.selectionPopup.hidden = true;
    this.anchor = null;
  }

  private async addHighlight(color: string) {
    if (!this.anchor) return;
    await gatewayRequest(
      "POST",
      `/bib/articles/${encodeURIComponent(this.articleId)}/annotations`,
      {
        content: this.anchor.text,
        kind: "highlight",
        page: this.anchor.page,
        color,
        rects: this.anchor.rects,
      },
    );
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    await this.loadAnnotations();
    this.renderSidebarTags();
  }

  private async addNote() {
    if (!this.anchor) return;
    const comment = window.prompt("Add note");
    if (!comment?.trim()) return;
    await gatewayRequest(
      "POST",
      `/bib/articles/${encodeURIComponent(this.articleId)}/annotations`,
      {
        content: comment.trim(),
        kind: "comment",
        page: this.anchor.page,
        rects: this.anchor.rects,
      },
    );
    this.hideSelectionPopup();
    window.getSelection()?.removeAllRanges();
    await this.loadAnnotations();
    this.renderSidebarTags();
  }

  private async deleteAnnotation(id: string) {
    if (!window.confirm("Delete this annotation?")) return;
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

  private async searchPdf(term: string) {
    if (!this.document || !term) {
      this.searchResults = [];
      this.renderSearchResults();
      return;
    }
    this.setStatus(`Searching “${term}”…`);
    const needle = term.toLowerCase();
    const results: Array<{ page: number; count: number }> = [];
    for (let page = 1; page <= this.pageCount; page += 1) {
      const text = await this.pageText(page);
      const count = text.split(needle).length - 1;
      if (count > 0) results.push({ page, count });
    }
    this.searchResults = results;
    this.setStatus(`${results.length.toLocaleString()} pages match`);
    this.renderSearchResults();
    this.selectPane("search");
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
    this.fitMode = "custom";
    this.zoom = Math.min(Math.max(zoom, 0.2), 6);
    this.zoomLabel.textContent = `${Math.round(this.zoom * 100)}%`;
    await this.relayout();
  }

  private async refit() {
    if (!this.document || this.destroyed) return;
    if (this.fitMode !== "custom") {
      const proxy = await this.document.getPage(1);
      const base = proxy.getViewport({ scale: 1, rotation: this.rotation });
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
    this.setStatus("Rendering…");
    for (let page = 1; page <= this.pageCount; page += 1) {
      const proxy = await this.document.getPage(page);
      const base = proxy.getViewport({ scale: 1, rotation: this.rotation });
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
      view.text.replaceChildren();
      this.cancelRender(view);
      view.canvas.width = 0;
      view.canvas.height = 0;
      view.wrapper.classList.add("rendering");
    }

    this.renderVisiblePages();
  }

  private renderVisiblePages() {
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
    view.task = null;
    view.rendering = false;
  }

  private async renderPage(page: number) {
    const view = this.pageViews.get(page);
    const document = this.document;
    if (!document || this.destroyed || !view || view.rendering) return;
    const generation = view.generation;
    view.rendering = true;
    try {
      const proxy = await document.getPage(page);
      if (this.destroyed || view.generation !== generation) return;
      const viewport = proxy.getViewport({ scale: this.zoom, rotation: this.rotation });
      view.viewport = viewport;
      const outputScale = Math.min(window.devicePixelRatio || 1, 2);
      view.canvas.width = Math.floor(viewport.width * outputScale);
      view.canvas.height = Math.floor(viewport.height * outputScale);
      const context = view.canvas.getContext("2d")!;
      context.setTransform(outputScale, 0, 0, outputScale, 0, 0);
      context.clearRect(0, 0, viewport.width, viewport.height);

      const task = proxy.render({
        canvasContext: context,
        viewport,
      });
      view.task = task;
      await task.promise;
      if (this.destroyed || view.generation !== generation) return;

      view.text.replaceChildren();
      view.text.style.setProperty("--scale-factor", String(this.zoom));
      const textContent = await proxy.getTextContent();
      if (this.destroyed || view.generation !== generation) return;
      const textLayer = new TextLayer({
        textContentSource: textContent,
        container: view.text,
        viewport,
      });
      await textLayer.render();
      view.wrapper.classList.remove("rendering");
      if (page === this.currentPage) {
        this.setStatus(`${page.toLocaleString()} / ${this.pageCount.toLocaleString()}`);
      }
    } catch (error) {
      if ((error as any)?.name !== "RenderingCancelledException") {
        this.setStatus(`Render failed: ${String(error)}`);
      }
    } finally {
      view.rendering = false;
      view.task = null;
    }
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    if (this.relayoutTimer !== null) window.clearTimeout(this.relayoutTimer);
    this.resizeObserver?.disconnect();
    this.pageObserver?.disconnect();
    for (const view of this.pageViews.values()) {
      this.cancelRender(view);
      view.text.replaceChildren();
    }
    this.document?.destroy().catch(() => undefined);
    this.root.remove();
    if (activeReader) activeReader = null;
  }
}
