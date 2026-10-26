import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import { TextLayer } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { fetchBibFulltextRaw } from "./gateway";
import { escapeHtml } from "./markdown";
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
  task: RenderTask | null;
  rendering: boolean;
  generation: number;
};

export type PdfReaderHandle = {
  close: () => void;
};

let activeReader: PdfReaderHandle | null = null;

export async function openPdfReader(options: {
  articleId: string;
  title: string;
}): Promise<PdfReaderHandle> {
  activeReader?.close();
  const reader = await ZoteroStylePdfReader.open(options.articleId, options.title);
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
  private outlinePane: HTMLDivElement;
  private thumbnailsPane: HTMLDivElement;

  private constructor(private readonly title: string) {
    this.root = document.createElement("div");
    this.root.className = "zotero-pdf-reader";
    this.root.innerHTML = `
      <aside class="reader-sidebar">
        <div class="reader-sidebar-tabs">
          <button type="button" data-pane="thumbnails" class="active">Pages</button>
          <button type="button" data-pane="outline">Outline</button>
        </div>
        <div class="reader-sidebar-pane" data-pane="thumbnails"></div>
        <div class="reader-sidebar-pane" data-pane="outline" hidden></div>
      </aside>
      <main class="reader-main">
        <header class="reader-toolbar">
          <button type="button" data-action="sidebar" title="Toggle sidebar">☰</button>
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
          <button type="button" data-action="rotate">Rotate</button>
          <div class="reader-title">${escapeHtml(this.title)}</div>
          <button type="button" data-action="close" title="Close">✕</button>
        </header>
        <div class="reader-page-area">
          <div class="reader-pages"></div>
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
    this.outlinePane = this.select('[data-pane="outline"]');
    this.thumbnailsPane = this.select('[data-pane="thumbnails"]');

    this.bindEvents();
  }

  static async open(
    articleId: string,
    title: string,
  ): Promise<ZoteroStylePdfReader> {
    const reader = new ZoteroStylePdfReader(title);
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
    action("sidebar", () => this.root.classList.toggle("sidebar-collapsed"));

    this.pageNumberInput.addEventListener("change", () => {
      void this.gotoPage(Number(this.pageNumberInput.value));
    });

    this.root
      .querySelectorAll<HTMLButtonElement>(".reader-sidebar-tabs button")
      .forEach((button) => {
        button.addEventListener("click", () => {
          this.root
            .querySelectorAll<HTMLButtonElement>(".reader-sidebar-tabs button")
            .forEach((item) => item.classList.toggle("active", item === button));
          const pane = button.dataset.pane ?? "thumbnails";
          this.outlinePane.hidden = pane !== "outline";
          this.thumbnailsPane.hidden = pane !== "thumbnails";
        });
      });

    this.pageArea.addEventListener("scroll", () => this.updateCurrentFromScroll());
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
    await Promise.all([this.loadThumbnails(), this.loadOutline()]);
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
      this.pageViews.set(page, { wrapper, canvas, text, task: null, rendering: false, generation: 0 });
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
      const viewport = proxy.getViewport({ scale: 132 / base.width });
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
