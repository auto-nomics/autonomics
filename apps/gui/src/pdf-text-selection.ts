/** Keep native selection endpoints next to PDF text instead of the page-sized
 * absolutely positioned container. TextLayer alone does not provide the
 * selection boundary used by PDF.js's TextLayerBuilder. Without it, Chromium
 * selects most of the page whenever a drag crosses the whitespace between lines.
 */
export class PdfTextSelection {
  private readonly boundaries = new Map<HTMLElement, HTMLDivElement>();
  private readonly events = new AbortController();
  private dragging = false;

  constructor(private readonly container: HTMLElement) {
    const { signal } = this.events;
    container.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      const layer = (event.target as Element).closest<HTMLElement>('.reader-text-layer');
      if (!layer || !this.boundaries.has(layer)) return;
      this.dragging = true;
      container.classList.add('reader-selecting');
      layer.classList.add('selecting');
    }, { signal });
    document.addEventListener('selectionchange', () => this.update(), { signal });
    document.addEventListener('pointerup', () => this.finish(), { signal });
    document.addEventListener('pointercancel', () => this.finish(), { signal });
    window.addEventListener('blur', () => this.finish(), { signal });
    document.addEventListener('keyup', () => {
      if (!this.dragging) this.resetBoundaries();
    }, { signal });
  }

  register(layer: HTMLElement) {
    const boundary = document.createElement('div');
    boundary.className = 'reader-text-boundary';
    boundary.setAttribute('aria-hidden', 'true');
    layer.append(boundary);
    this.boundaries.set(layer, boundary);
  }

  private resetBoundary(layer: HTMLElement, boundary: HTMLElement) {
    // Do not keep moving a node that is already in place: even an empty DOM
    // mutation can cause another selectionchange while the user is dragging.
    if (boundary.parentElement !== layer || boundary !== layer.lastChild) layer.append(boundary);
    boundary.style.width = '';
    boundary.style.height = '';
    layer.classList.remove('selecting');
  }

  private resetBoundaries() {
    for (const [layer, boundary] of this.boundaries) this.resetBoundary(layer, boundary);
  }

  private finish() {
    this.dragging = false;
    this.container.classList.remove('reader-selecting');
    this.resetBoundaries();
  }

  private update() {
    const selection = document.getSelection();
    if (!selection?.rangeCount || selection.isCollapsed) {
      if (!this.dragging) this.resetBoundaries();
      return;
    }
    const range = selection.getRangeAt(0);
    for (const [layer, boundary] of this.boundaries) {
      if (range.intersectsNode(layer)) layer.classList.add('selecting');
      else this.resetBoundary(layer, boundary);
    }
    // Firefox already resolves gaps between absolutely positioned text runs.
    if (CSS.supports('-moz-user-select', 'none')) return;
    const focus = selection.focusNode;
    const element = focus?.nodeType === Node.TEXT_NODE ? focus.parentElement : focus as Element | null;
    const text = element?.closest<HTMLElement>('span');
    const layer = text?.closest<HTMLElement>('.reader-text-layer');
    const boundary = layer && this.boundaries.get(layer);
    if (!text || !layer || !boundary) return;
    const backwards = focus === range.startContainer && selection.focusOffset === range.startOffset;
    boundary.style.width = layer.style.width;
    boundary.style.height = layer.style.height;
    const next = backwards ? text : text.nextSibling;
    if (next !== boundary && (boundary.parentNode !== text.parentNode || boundary.nextSibling !== next)) text.parentNode!.insertBefore(boundary, next);
  }

  clear() {
    this.finish();
    for (const boundary of this.boundaries.values()) boundary.remove();
    this.boundaries.clear();
  }

  destroy() {
    this.events.abort();
    this.clear();
  }
}
