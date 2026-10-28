import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
import { highlightNoteIndicator } from './highlight-note-indicator';
import { TextQuoteIndex, normalizeQuote, type ReadingTextRange } from './reading-text-range';
import { ReadingImagePreview } from './reading-image-preview';
import {
  renderSDT, PDFPositionMapper, getBlockNodeByRef,
  type StructuredDocumentText, type SDTPosition, type PDFPosition, type ContentBlockNode,
} from '@/vendor/zotero-sdt.generated';

export type ReadingAppearance = {
  fontSize: number;
  fontFamily: 'serif' | 'sans';
  lineHeight: number;
  wordSpacing: number;
  letterSpacing: number;
  pageWidth: 'narrow' | 'normal' | 'full';
  theme: 'light' | 'sepia' | 'dark';
};
export const DEFAULT_READING_APPEARANCE: ReadingAppearance = {
  fontSize: 18, fontFamily: 'serif', lineHeight: 1.2, wordSpacing: 0,
  letterSpacing: 0, pageWidth: 'normal', theme: 'light',
};
export type ReadingAnchor = { ref: string; fraction: number };
type Annotation = { id: string; content?: string; comment?: string; kind?: string; color?: string; page?: number; rects?: Array<{ left: number; top: number; width: number; height: number; space?: string; text_range?: ReadingTextRange }> };

export function sourcePageRects(block: ContentBlockNode | null | undefined): number[][] {
  const anchor = block?.anchor;
  return anchor && "pageRects" in anchor ? anchor.pageRects ?? [] : [];
}

/** Host adapter around Zotero's semantic renderer and exact PDF/SDT mapper. */
export class ReadingModeView {
  readonly article: HTMLElement;
  readonly mapper: PDFPositionMapper;
  private overlay = document.createElement('div');
  private observer: ResizeObserver;
  private cropObserver: IntersectionObserver;
  private tasks = new Set<RenderTask>();
  private annotations: Annotation[] = [];
  private quoteIndex: TextQuoteIndex | null = null;
  private disposed = false;
  private frame = 0;
  private imagePreview = new ReadingImagePreview();
  private imageSources = new WeakMap<HTMLCanvasElement, { page: number; rect: number[] }>();

  constructor(
    readonly container: HTMLElement,
    readonly structure: StructuredDocumentText,
    private pdf: PDFDocumentProxy,
    private onLocation: (page: number) => void,
    private onSelectAnnotation: (id: string) => void = () => {},
  ) {
    this.mapper = new PDFPositionMapper(structure);
    this.article = renderSDT(structure, document, { renderSourceCrops: true });
    this.article.classList.add('reader-reflow-article');
    this.article.lang = String(structure.metadata.source.properties?.Language ?? '');
    this.overlay.className = 'reader-reflow-overlays';
    container.replaceChildren(this.article, this.overlay);
    // Semantic source links stay in the reader; allow only ordinary web/mail URLs.
    this.article.querySelectorAll<HTMLAnchorElement>('a').forEach(link => {
      if (link.classList.contains('sdt-ref')) {
        link.addEventListener('click', event => {
          event.preventDefault();
          const target = this.article.querySelector<HTMLElement>(`#${CSS.escape(link.hash.slice(1))}`);
          if (target) this.scrollTo(target);
        });
      } else if (/^https?:|^mailto:/i.test(link.href)) {
        link.target = '_blank'; link.rel = 'noopener noreferrer';
      } else link.removeAttribute('href');
    });
    this.observer = new ResizeObserver(() => this.scheduleOverlays());
    this.observer.observe(this.article);
    this.cropObserver = new IntersectionObserver(entries => {
      for (const entry of entries) if (entry.isIntersecting) {
        this.cropObserver.unobserve(entry.target);
        void this.renderCrop(entry.target as HTMLElement);
      }
    }, { root: container, rootMargin: '600px' });
    this.article.querySelectorAll<HTMLElement>('.sdt-source-crop').forEach(figure => {
      const label = document.createElement('span');
      label.className = 'reader-crop-status'; label.textContent = 'Loading original figure…';
      figure.append(label);
      this.cropObserver.observe(figure);
    });
    this.article.querySelectorAll<HTMLImageElement>('img').forEach(image => this.prepareImage(image));
    this.article.addEventListener('click', event => {
      const source = (event.target as Element).closest<HTMLCanvasElement | HTMLImageElement>('.reader-zoomable-image');
      if (source) { event.preventDefault(); event.stopPropagation(); this.openImage(source); }
    });
    this.article.addEventListener('keydown', event => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      const source = (event.target as Element).closest<HTMLCanvasElement | HTMLImageElement>('.reader-zoomable-image');
      if (source) { event.preventDefault(); event.stopPropagation(); this.openImage(source); }
    });
    container.addEventListener('scroll', this.handleScroll, { passive: true });
  }

  setAppearance(value: ReadingAppearance) {
    this.container.dataset.readingTheme = value.theme;
    this.article.dataset.pageWidth = value.pageWidth;
    this.article.style.setProperty('--reading-font-size', `${value.fontSize}px`);
    this.article.style.setProperty('--reading-font-family', value.fontFamily === 'serif' ? 'Georgia, "Noto Serif CJK SC", "Songti SC", serif' : 'system-ui, sans-serif');
    this.article.style.setProperty('--reading-line-height', String(value.lineHeight));
    this.article.style.setProperty('--reading-word-spacing', `${value.wordSpacing / 100}em`);
    this.article.style.setProperty('--reading-letter-spacing', `${value.letterSpacing}em`);
    this.scheduleOverlays();
  }

  private handleScroll = () => {
    const anchor = this.captureAnchor(this.container.clientHeight / 2);
    const block = anchor && getBlockNodeByRef(this.structure.content, anchor.ref.split('.').map(Number));
    const page = sourcePageRects(block)[0]?.[0];
    if (typeof page === 'number') this.onLocation(page + 1);
    this.scheduleOverlays();
  };

  captureAnchor(offset = 24): ReadingAnchor | null {
    const top = this.container.getBoundingClientRect().top + offset;
    for (const block of this.article.querySelectorAll<HTMLElement>('[data-ref-path]')) {
      const rect = block.getBoundingClientRect();
      if (rect.height > 0 && rect.bottom > top) {
        return { ref: block.dataset.refPath!, fraction: Math.max(0, Math.min(1, (top - rect.top) / rect.height)) };
      }
    }
    return null;
  }

  restoreAnchor(anchor: ReadingAnchor | null) {
    if (!anchor) return;
    const block = this.article.querySelector<HTMLElement>(`[data-ref-path="${anchor.ref}"]`);
    if (!block) return;
    this.container.scrollTop += block.getBoundingClientRect().top - this.container.getBoundingClientRect().top
      + block.getBoundingClientRect().height * anchor.fraction - 24;
  }

  gotoPage(page: number, pdfTop?: number) {
    const candidates = [...this.article.querySelectorAll<HTMLElement>('[data-ref-path]')];
    let first: HTMLElement | undefined;
    let closest: HTMLElement | undefined;
    let distance = Infinity;
    for (const element of candidates) {
      const node = getBlockNodeByRef(this.structure.content, element.dataset.refPath!.split('.').map(Number));
      for (const rect of sourcePageRects(node)) if (rect[0] === page - 1) {
        first ??= element;
        if (pdfTop !== undefined && Math.abs(rect[4] - pdfTop) < distance) {
          distance = Math.abs(rect[4] - pdfTop); closest = element;
        }
      }
    }
    if (closest ?? first) this.scrollTo((closest ?? first)!);
  }

  private scrollTo(element: HTMLElement) {
    this.container.scrollTop += element.getBoundingClientRect().top - this.container.getBoundingClientRect().top - 24;
    this.handleScroll();
  }

  /** Text-node offsets survive font/width/theme changes; no screen coordinates
   * are ever persisted as PDF annotations in Reading Mode. */
  selection(range: Range): { position: PDFPosition; text: string; rect: DOMRect; textRange: ReadingTextRange } | null {
    if (!this.article.contains(range.startContainer) || !this.article.contains(range.endContainer)) return null;
    const start = this.domPoint(range.startContainer, range.startOffset, false);
    const end = this.domPoint(range.endContainer, range.endOffset, true);
    if (!start || !end) return null;
    const position = this.mapper.sdtToSourcePosition({ start, end });
    if (!position) return null;
    const text = normalizeQuote(range.toString());
    return { position, text, rect: range.getBoundingClientRect(), textRange: { source_hash: this.structure.metadata.source.hash, start, end, quote: text } };
  }

  textRangeOnPage(selection: ReadingTextRange, position: PDFPosition): ReadingTextRange | null {
    const projected = this.mapper.sourceToSDTPosition(position);
    if (!projected) return null;
    const compare = (a: number[], b: number[]) => {
      for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i];
      return a.length - b.length;
    };
    const start = compare(selection.start, projected.start) > 0 ? selection.start : projected.start;
    const end = compare(selection.end, projected.end) < 0 ? selection.end : projected.end;
    if (compare(start, end) >= 0) return null;
    const range = this.range({ start, end });
    return range && { ...selection, start, end, quote: normalizeQuote(range.toString()) };
  }

  private domPoint(node: Node, offset: number, end: boolean): number[] | null {
    let element = node.nodeType === Node.ELEMENT_NODE ? node as HTMLElement : node.parentElement;
    let span = element?.closest<HTMLElement>('[data-text-index]');
    if (!span && node.nodeType === Node.ELEMENT_NODE) {
      const range = document.createRange(); range.setStart(node, offset); range.collapse(true);
      const spans = [...(element?.querySelectorAll<HTMLElement>('[data-text-index]') ?? [])];
      span = (end ? spans.reverse() : spans).find(candidate => {
        const test = document.createRange(); test.selectNodeContents(candidate);
        return end ? test.compareBoundaryPoints(Range.END_TO_END, range) <= 0 : test.compareBoundaryPoints(Range.START_TO_START, range) >= 0;
      });
    }
    const block = span?.closest<HTMLElement>('[data-ref-path]');
    if (!span || !block || !this.article.contains(span)) return null;
    let count = end ? span.textContent?.length ?? 0 : 0;
    if (span.contains(node)) {
      const range = document.createRange(); range.setStart(span, 0); range.setEnd(node, offset); count = range.toString().length;
    }
    return [...block.dataset.refPath!.split('.').map(Number), Number(span.dataset.textIndex), count];
  }

  private range(position: SDTPosition): Range | null {
    const point = (value: number[]) => {
      if (value.length < 3 || !value.every(offset => Number.isSafeInteger(offset) && offset >= 0)) return null;
      const block = this.article.querySelector<HTMLElement>(`[data-ref-path="${value.slice(0, -2).join('.')}"]`);
      const span = block?.querySelector(`[data-text-index="${value.at(-2)}"]`);
      if (!span) return null;
      let remaining = value.at(-1)!;
      const walker = document.createTreeWalker(span, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        const node = walker.currentNode as Text;
        if (remaining <= node.length) return { node, offset: remaining };
        remaining -= node.length;
      }
      return null;
    };
    const start = point(position.start), end = point(position.end);
    if (!start || !end) return null;
    const range = document.createRange(); range.setStart(start.node, start.offset); range.setEnd(end.node, end.offset);
    return range.collapsed ? null : range;
  }

  setAnnotations(annotations: Annotation[]) { this.annotations = annotations; this.scheduleOverlays(); }

  private annotationRange(annotation: Annotation): Range | null {
    const exact = annotation.rects?.find(rect => rect.text_range)?.text_range;
    if (exact && exact.source_hash === this.structure.metadata.source.hash) {
      const range = this.range(exact);
      if (range && normalizeQuote(range.toString()) === exact.quote) return range;
    }
    const pageIndex = Number(annotation.page ?? 1) - 1;
    const page = this.structure.catalog.pages[pageIndex];
    if (!page || !annotation.rects?.length) return null;
    if (!page.viewRect) return null;
    const [x0, y0, x1, y1] = page.viewRect;
    const rects = annotation.rects.filter(rect => rect.space === 'pdf').map(rect => [
      x0 + rect.left * (x1 - x0), y0 + rect.top * (y1 - y0),
      x0 + (rect.left + rect.width) * (x1 - x0), y0 + (rect.top + rect.height) * (y1 - y0),
    ]);
    const position = this.mapper.sourceToSDTPosition({ pageIndex, rects });
    const projected = position && this.range(position);
    if (!projected || annotation.kind !== 'highlight' || !annotation.content) return projected;
    this.quoteIndex ??= new TextQuoteIndex(this.article);
    return this.quoteIndex.find(annotation.content, projected) ?? projected;
  }

  navigateAnnotation(annotation: Annotation) {
    const range = this.annotationRange(annotation);
    if (range) {
      this.container.scrollTop += range.getBoundingClientRect().top - this.container.getBoundingClientRect().top - 40;
      this.handleScroll();
    } else this.gotoPage(Number(annotation.page ?? 1));
  }

  private scheduleOverlays() {
    if (this.frame || this.disposed) return;
    this.frame = requestAnimationFrame(() => { this.frame = 0; this.drawAnnotations(); });
  }

  private drawAnnotations() {
    this.overlay.replaceChildren();
    const origin = this.container.getBoundingClientRect();
    const fragment = document.createDocumentFragment();
    for (const annotation of this.annotations) {
      const isNote = annotation.kind === 'note' || annotation.kind === 'comment';
      if (annotation.kind !== 'highlight' && !isNote) continue;
      const range = this.annotationRange(annotation);
      if (!range) continue;
      const rects = [...range.getClientRects()].filter(rect => rect.width > 0 && rect.height > 0 && rect.bottom >= origin.top && rect.top <= origin.bottom);
      for (const rect of (isNote ? rects.slice(0, 1) : rects)) {
        const mark = document.createElement('div');
        mark.className = isNote ? 'reader-reflow-highlight reader-note-marker' : 'reader-reflow-highlight'; mark.dataset.annotationId = annotation.id;
        if (isNote) { mark.textContent = '✎'; mark.title = 'Note'; }
        Object.assign(mark.style, {
          left: `${rect.left - origin.left + this.container.scrollLeft}px`,
          top: `${rect.top - origin.top + this.container.scrollTop}px`,
          width: `${rect.width}px`, height: `${rect.height}px`,
          background: annotation.color ?? '#ffd400',
        });
        fragment.append(mark);
      }
      if (!isNote && rects.length) {
        const indicator = highlightNoteIndicator(annotation, this.onSelectAnnotation);
        if (indicator) {
          const first = rects[0];
          Object.assign(indicator.style, {
            left: `${this.container.scrollLeft + Math.min(this.container.clientWidth - 12, first.right - origin.left + 17)}px`,
            top: `${this.container.scrollTop + Math.max(12, Math.min(this.container.clientHeight - 12, first.top - origin.top + first.height / 2))}px`,
          });
          fragment.append(indicator);
        }
      }
    }
    this.overlay.append(fragment);
  }

  private async renderCrop(figure: HTMLElement) {
    const ref = figure.dataset.refPath!.split('.').map(Number);
    const block = getBlockNodeByRef(this.structure.content, ref);
    const byPage = new Map<number, number[]>();
    for (const [page, x0, y0, x1, y1] of sourcePageRects(block)) {
      const existing = byPage.get(page);
      byPage.set(page, existing ? [Math.min(existing[0], x0), Math.min(existing[1], y0), Math.max(existing[2], x1), Math.max(existing[3], y1)] : [x0, y0, x1, y1]);
    }
    try {
      for (const [index, rect] of byPage) {
        const canvas = await this.renderCropCanvas(index, rect, 2, 4_000_000);
        if (this.disposed) return;
        this.imageSources.set(canvas, { page: index, rect });
        this.prepareImage(canvas);
        figure.querySelector('.sdt-source-crop-pages')?.append(canvas);
      }
      const status = figure.querySelector('.reader-crop-status');
      if (byPage.size) status?.remove();
      else if (status) status.textContent = 'Source figure is unavailable';
      this.scheduleOverlays();
    } catch {
      if (!this.disposed) {
        const status = figure.querySelector('.reader-crop-status');
        if (status) status.textContent = 'Could not render figure. View it in the original PDF.';
      }
    }
  }

  private prepareImage(source: HTMLCanvasElement | HTMLImageElement) {
    source.classList.add('reader-zoomable-image');
    source.tabIndex = 0;
    source.setAttribute('role', 'button');
    source.setAttribute('aria-haspopup', 'dialog');
    source.setAttribute('aria-label', 'Enlarge image');
    source.title = 'Click to enlarge';
  }

  private openImage(source: HTMLCanvasElement | HTMLImageElement) {
    const crop = source instanceof HTMLCanvasElement ? this.imageSources.get(source) : undefined;
    this.imagePreview.open(source, crop
      ? signal => this.renderCropCanvas(crop.page, crop.rect, 4, 16_000_000, signal)
      : undefined);
  }

  private async renderCropCanvas(index: number, rect: number[], maxScale: number, maxPixels: number, signal?: AbortSignal): Promise<HTMLCanvasElement> {
    const page = await this.pdf.getPage(index + 1);
    if (this.disposed || signal?.aborted) throw new DOMException('Image rendering cancelled', 'AbortError');
    const base = page.getViewport({ scale: 1 }).convertToViewportRectangle(rect);
    const area = Math.max(1, Math.abs((base[2] - base[0]) * (base[3] - base[1])));
    const scale = Math.min(maxScale, Math.sqrt(maxPixels / area));
    const viewport = page.getViewport({ scale });
    const bounds = viewport.convertToViewportRectangle(rect);
    const left = Math.min(bounds[0], bounds[2]), top = Math.min(bounds[1], bounds[3]);
    const canvas = document.createElement('canvas');
    canvas.width = Math.ceil(Math.abs(bounds[2] - bounds[0]));
    canvas.height = Math.ceil(Math.abs(bounds[3] - bounds[1]));
    if (!canvas.width || !canvas.height) throw new Error('Empty image region');
    canvas.style.width = `${canvas.width / scale}px`;
    const task = page.render({ canvasContext: canvas.getContext('2d')!, viewport, transform: [1, 0, 0, 1, -left, -top] });
    const cancel = () => task.cancel();
    signal?.addEventListener('abort', cancel, { once: true });
    this.tasks.add(task);
    try { await task.promise; } finally {
      this.tasks.delete(task);
      signal?.removeEventListener('abort', cancel);
    }
    return canvas;
  }

  destroy() {
    this.disposed = true;
    this.imagePreview.destroy();
    this.container.removeEventListener('scroll', this.handleScroll);
    this.observer.disconnect(); this.cropObserver.disconnect();
    if (this.frame) cancelAnimationFrame(this.frame);
    for (const task of this.tasks) task.cancel();
    this.container.replaceChildren();
  }
}
