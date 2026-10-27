type PreviewSource = HTMLCanvasElement | HTMLImageElement;
type RenderPreview = (signal: AbortSignal) => Promise<HTMLCanvasElement>;

/** A modal image viewer, independent of document zoom and reading position. */
export class ReadingImagePreview {
  private dialog = document.createElement('dialog');
  private viewport: HTMLDivElement;
  private stage: HTMLDivElement;
  private output: HTMLOutputElement;
  private media: PreviewSource | null = null;
  private source: PreviewSource | null = null;
  private request: AbortController | null = null;
  private width = 1;
  private height = 1;
  private scale = 1;
  private fitting = true;
  private drag: { id: number; x: number; y: number; left: number; top: number } | null = null;
  private resizeObserver: ResizeObserver;

  constructor() {
    this.dialog.className = 'reader-image-preview';
    this.dialog.setAttribute('aria-label', 'Image preview');
    this.dialog.innerHTML = `
      <header class="reader-image-preview-toolbar">
        <strong>Image preview</strong>
        <button type="button" data-image-action="out" aria-label="Zoom out">−</button>
        <output aria-live="polite" aria-label="Image zoom">100%</output>
        <button type="button" data-image-action="in" aria-label="Zoom in">+</button>
        <button type="button" data-image-action="fit">Fit to window</button>
        <button type="button" data-image-action="actual">100%</button>
        <button type="button" data-image-action="close" aria-label="Close image preview">✕</button>
      </header>
      <div class="reader-image-preview-viewport" tabindex="0" aria-label="Image; drag to pan, scroll to zoom">
        <div class="reader-image-preview-stage"></div>
      </div>
      <p class="reader-image-preview-hint">Scroll to zoom · Drag to pan · Esc to close</p>`;
    this.viewport = this.dialog.querySelector('.reader-image-preview-viewport')!;
    this.stage = this.dialog.querySelector('.reader-image-preview-stage')!;
    this.output = this.dialog.querySelector('output')!;
    this.dialog.querySelectorAll<HTMLButtonElement>('[data-image-action]').forEach(button => {
      button.addEventListener('click', () => {
        switch (button.dataset.imageAction) {
          case 'close': this.close(); break;
          case 'fit': this.fit(); break;
          case 'actual': this.zoom(1); break;
          case 'in': this.zoom(this.scale * 1.25); break;
          case 'out': this.zoom(this.scale / 1.25); break;
        }
      });
    });
    this.dialog.addEventListener('cancel', event => { event.preventDefault(); this.close(); });
    this.dialog.addEventListener('pointerdown', event => {
      const rect = this.dialog.getBoundingClientRect();
      if (event.target === this.dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) this.close();
    });
    this.dialog.addEventListener('keydown', event => {
      if (['+', '=', '-', '0', '1'].includes(event.key)) {
        event.preventDefault();
        if (event.key === '0') this.fit();
        else if (event.key === '1') this.zoom(1);
        else this.zoom(this.scale * (event.key === '-' ? .8 : 1.25));
      }
    });
    this.viewport.addEventListener('wheel', event => {
      event.preventDefault(); event.stopPropagation();
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? this.viewport.clientHeight : 1);
      this.zoom(this.scale * Math.exp(-Math.max(-300, Math.min(300, delta)) * .002), event.clientX, event.clientY);
    }, { passive: false });
    this.viewport.addEventListener('pointerdown', event => {
      if (event.button !== 0 || !event.isPrimary) return;
      event.preventDefault();
      this.drag = { id: event.pointerId, x: event.clientX, y: event.clientY, left: this.viewport.scrollLeft, top: this.viewport.scrollTop };
      this.viewport.setPointerCapture(event.pointerId);
      this.viewport.classList.add('dragging');
    });
    this.viewport.addEventListener('pointermove', event => {
      if (!this.drag || this.drag.id !== event.pointerId) return;
      this.viewport.scrollLeft = this.drag.left + this.drag.x - event.clientX;
      this.viewport.scrollTop = this.drag.top + this.drag.y - event.clientY;
    });
    const stop = () => { this.drag = null; this.viewport.classList.remove('dragging'); };
    this.viewport.addEventListener('pointerup', stop);
    this.viewport.addEventListener('pointercancel', stop);
    this.viewport.addEventListener('lostpointercapture', stop);
    this.resizeObserver = new ResizeObserver(() => {
      if (this.dialog.open) {
        if (this.fitting) this.fit();
        else this.layout();
      }
    });
    this.resizeObserver.observe(this.viewport);
  }

  open(source: PreviewSource, render?: RenderPreview) {
    this.close(false);
    this.source = source;
    this.width = source instanceof HTMLCanvasElement
      ? Number.parseFloat(source.style.width) || source.width
      : source.naturalWidth;
    this.height = source instanceof HTMLCanvasElement
      ? this.width * source.height / source.width
      : source.naturalHeight;
    if (!(this.width > 0 && this.height > 0)) return;
    if (source instanceof HTMLCanvasElement) {
      const canvas = document.createElement('canvas');
      canvas.width = source.width; canvas.height = source.height;
      canvas.getContext('2d')!.drawImage(source, 0, 0);
      this.media = canvas;
    } else {
      this.media = source.cloneNode() as HTMLImageElement;
      this.media.removeAttribute('id');
    }
    this.media.setAttribute('role', 'img');
    this.media.setAttribute('aria-label', source.getAttribute('aria-label') || source.closest('figure')?.getAttribute('aria-label') || 'Enlarged image');
    this.stage.replaceChildren(this.media);
    document.body.append(this.dialog);
    this.dialog.showModal();
    this.fit();
    this.dialog.querySelector<HTMLButtonElement>('[data-image-action=close]')!.focus();
    if (render) {
      const request = this.request = new AbortController();
      this.dialog.setAttribute('aria-busy', 'true');
      void render(request.signal).then(canvas => {
        if (request.signal.aborted || !this.dialog.open) return;
        canvas.setAttribute('role', 'img');
        canvas.setAttribute('aria-label', this.media!.getAttribute('aria-label')!);
        this.media = canvas;
        this.stage.replaceChildren(canvas);
        this.layout();
      }).catch(() => { /* Keep the existing preview if higher-resolution rendering fails. */ }).finally(() => {
        if (this.request === request) this.dialog.removeAttribute('aria-busy');
      });
    }
  }

  private fit() {
    this.fitting = true;
    this.scale = Math.max(.05, Math.min(8, (this.viewport.clientWidth - 48) / this.width, (this.viewport.clientHeight - 48) / this.height));
    this.layout();
    this.viewport.scrollLeft = this.viewport.scrollTop = 0;
  }

  private zoom(scale: number, clientX?: number, clientY?: number) {
    if (!this.media) return;
    this.fitting = false;
    const viewport = this.viewport.getBoundingClientRect();
    const x = clientX ?? viewport.left + viewport.width / 2;
    const y = clientY ?? viewport.top + viewport.height / 2;
    const before = this.media.getBoundingClientRect();
    const pointX = (x - before.left) / this.scale, pointY = (y - before.top) / this.scale;
    this.scale = Math.max(.05, Math.min(8, scale));
    this.layout();
    const after = this.media.getBoundingClientRect();
    this.viewport.scrollLeft += after.left + pointX * this.scale - x;
    this.viewport.scrollTop += after.top + pointY * this.scale - y;
  }

  private layout() {
    if (!this.media) return;
    const width = this.width * this.scale, height = this.height * this.scale;
    const stageWidth = Math.max(this.viewport.clientWidth, width + 48);
    const stageHeight = Math.max(this.viewport.clientHeight, height + 48);
    Object.assign(this.stage.style, { width: `${stageWidth}px`, height: `${stageHeight}px` });
    Object.assign(this.media.style, { width: `${width}px`, height: `${height}px`, left: `${(stageWidth - width) / 2}px`, top: `${(stageHeight - height) / 2}px` });
    this.output.value = `${Math.round(this.scale * 100)}%`;
    this.dialog.querySelector<HTMLButtonElement>('[data-image-action=out]')!.disabled = this.scale <= .05;
    this.dialog.querySelector<HTMLButtonElement>('[data-image-action=in]')!.disabled = this.scale >= 8;
  }

  close(restoreFocus = true) {
    this.request?.abort(); this.request = null;
    this.drag = null; this.viewport.classList.remove('dragging');
    this.dialog.removeAttribute('aria-busy');
    if (this.dialog.open) this.dialog.close();
    this.dialog.remove();
    this.stage.replaceChildren(); this.media = null;
    if (restoreFocus && this.source?.isConnected) this.source.focus({ preventScroll: true });
    this.source = null;
  }

  destroy() { this.close(false); this.resizeObserver.disconnect(); }
}
