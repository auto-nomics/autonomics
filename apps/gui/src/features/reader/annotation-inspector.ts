import type { ReadingTextRange } from './reading-text-range';
import { escapeHtml } from '@/lib/markdown';
import { AnnotationHoverCard } from './annotation-hover-card';

export type ReaderAnnotation = {
  id: string; content: string; kind?: string; page?: number; color?: string;
  comment?: string; tags?: string[];
  rects?: Array<{ left: number; top: number; width: number; height: number; space?: string; text_range?: ReadingTextRange }>;
};
export type AnnotationPatch = { comment?: string; tags?: string[]; color?: string };

/** Zotero's selected-annotation/PopupPreview interaction, adapted to our API.
 * Hit-test painted rectangles without placing click targets over selectable
 * text: dragging across an existing highlight must still select native text. */
export class AnnotationInspector {
  private annotations = new Map<string, ReaderAnnotation>();
  private selected: string | null = null;
  private popup = document.createElement('div');
  private outline = document.createElement('div');
  private hoverCard: AnnotationHoverCard;
  private commitTagInput: (() => void) | null = null;
  private events = new AbortController();
  private mutations: MutationObserver;
  private resize: ResizeObserver;
  private frame = 0;
  private timer: number | null = null;
  private pending = new Map<string, AnnotationPatch>();
  private drafts = new Map<string, AnnotationPatch>();
  private failed = new Set<string>();
  private queue: Promise<void> = Promise.resolve();
  private down: { x: number; y: number; id: string | null } | null = null;
  private disposed = false;

  constructor(
    private root: HTMLElement,
    private update: (id: string, patch: AnnotationPatch) => Promise<ReaderAnnotation>,
    private remove: (id: string) => Promise<void>,
    private clearTextSelection: () => void,
  ) {
    this.popup.className = 'reader-annotation-editor';
    this.outline.className = 'reader-annotation-outline'; this.outline.hidden = true;
    root.append(this.outline);
    this.hoverCard = new AnnotationHoverCard(root, this.popup, () => this.close());
    const { signal } = this.events;
    root.addEventListener('pointerdown', event => {
      if (event.button !== 0 || !event.isPrimary || (event.target as Element).closest('.reader-annotation-popup')) return;
      if (root.dataset.annotationTool === 'highlight' || root.dataset.annotationTool === 'note') { this.down = null; this.close(); return; }
      if (!(event.target as Element).closest('.reader-page-area,.reader-reflow')) { this.close(); return; }
      if (root.dataset.annotationTool && root.dataset.annotationTool !== 'pointer') {
        this.down = null; this.close(); return;
      }
      this.down = { x: event.clientX, y: event.clientY, id: this.hit(event.clientX, event.clientY) };
      this.close();
    }, { signal, capture: true });
    root.addEventListener('pointerup', event => {
      const down = this.down; this.down = null;
      if (!down?.id || event.button !== 0 || Math.hypot(event.clientX - down.x, event.clientY - down.y) > 4) return;
      if (!window.getSelection()?.isCollapsed || this.hit(event.clientX, event.clientY) !== down.id) return;
      this.select(down.id, { x: event.clientX, y: event.clientY });
    }, { signal });
    root.addEventListener('pointercancel', () => { this.down = null; }, { signal });
    document.addEventListener('pointerup', () => { this.down = null; }, { signal });
    document.addEventListener('keydown', event => {
      if (event.key === 'Escape' && this.selected) {
        event.preventDefault(); event.stopPropagation(); this.close();
      }
    }, { signal, capture: true });
    root.addEventListener('scroll', () => this.scheduleLayout(), { signal, capture: true });
    window.addEventListener('resize', () => this.scheduleLayout(), { signal });
    this.mutations = new MutationObserver(() => this.scheduleLayout());
    this.mutations.observe(root, { childList: true, subtree: true });
    this.resize = new ResizeObserver(() => this.scheduleLayout());
    this.resize.observe(root);
  }

  setAnnotations(annotations: ReaderAnnotation[]) {
    this.annotations = new Map(annotations.map(annotation => [annotation.id, annotation]));
    if (this.selected && !this.annotations.has(this.selected)) this.close();
    this.scheduleLayout();
  }

  private marks(id?: string): HTMLElement[] {
    return [...this.root.querySelectorAll<HTMLElement>('.reader-annotation,.reader-reflow-highlight')]
      .filter(mark => (!id || mark.dataset.annotationId === id) && mark.getBoundingClientRect().width > 0);
  }

  private hit(x: number, y: number): string | null {
    const candidates = this.marks().filter(mark => {
      const rect = mark.getBoundingClientRect();
      const area = mark.closest('.reader-page-area,.reader-reflow')?.getBoundingClientRect();
      return area && x >= area.left && x <= area.right && y >= area.top && y <= area.bottom
        && x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
    });
    // Prefer the most specific highlight when annotations overlap.
    candidates.sort((a, b) => a.offsetWidth * a.offsetHeight - b.offsetWidth * b.offsetHeight);
    return candidates[0]?.dataset.annotationId ?? null;
  }

  select(id: string, _point?: { x: number; y: number }) {
    const annotation = this.annotations.get(id);
    if (!annotation) return;
    this.commitTagInput?.();
    if (this.selected !== id) this.flush();
    this.selected = id;
    this.clearTextSelection();
    this.render(annotation);
    this.scheduleLayout();
  }

  private render(annotation: ReaderAnnotation) {
    const draft = this.drafts.get(annotation.id) ?? {};
    const comment = draft.comment ?? annotation.comment ?? '';
    const tags = draft.tags ?? annotation.tags ?? [];
    this.popup.dataset.annotationId = annotation.id;
    this.popup.innerHTML = `
      <header><span class="reader-annotation-icon" aria-hidden="true">A</span><span>Page ${Number(annotation.page ?? 1)}</span><button type="button" class="reader-annotation-more" aria-label="Annotation options" aria-expanded="false">•••</button></header>
      ${annotation.kind === 'note' || annotation.kind === 'comment' ? `<p class="reader-note-content">${escapeHtml(annotation.content)}</p>` : ''}
      <textarea aria-label="Annotation comment" placeholder="Add comment" rows="3">${escapeHtml(comment)}</textarea>
      <div class="reader-annotation-tags"></div>
      <input class="reader-annotation-tag-input" aria-label="Add annotation tags" placeholder="Add tags…" />
      <div class="reader-annotation-options" hidden>
        <div class="reader-annotation-colors" role="group" aria-label="Highlight color">${['#ffd400','#ff6666','#5fb236','#2ea8e5','#a28ae5','#e56eee','#f19837','#aaaaaa'].map(color => `<button type="button" data-annotation-color="${color}" aria-label="Color ${color}" style="--annotation-color:${color}"></button>`).join('')}</div>
        <button type="button" data-annotation-action="copy">Copy highlighted text</button>
        <button type="button" data-annotation-action="delete">Delete annotation</button>
      </div>
      <div class="reader-annotation-save-status" role="status"></div>`;
    this.popup.querySelector<HTMLElement>('.reader-annotation-icon')!.style.color = annotation.color ?? '#ffd400';
    this.renderTags(tags);
    if (this.failed.has(annotation.id)) this.status('Could not save. ', true);
    const textarea = this.popup.querySelector('textarea')!;
    textarea.addEventListener('input', () => this.stage({ comment: textarea.value }));
    textarea.addEventListener('blur', () => this.flush());
    textarea.addEventListener('keydown', event => {
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') { this.flush(); textarea.blur(); }
    });
    const tagInput = this.popup.querySelector<HTMLInputElement>('input')!;
    const commitTags = () => {
      const entered = tagInput.value.split(/[,，]/).map(tag => tag.trim()).filter(Boolean);
      if (!entered.length) return;
      const existing = this.drafts.get(annotation.id)?.tags ?? this.annotations.get(annotation.id)?.tags ?? [];
      const tags = [...new Set([...existing, ...entered])];
      tagInput.value = ''; this.stage({ tags }); this.renderTags(tags); this.flush();
    };
    this.commitTagInput = commitTags;
    tagInput.addEventListener('keydown', event => {
      if (event.key === 'Enter' || event.key === ',') { event.preventDefault(); commitTags(); }
    });
    tagInput.addEventListener('blur', commitTags);
    this.popup.querySelector('.reader-annotation-more')!.addEventListener('click', event => {
      const button = event.currentTarget as HTMLButtonElement;
      const options = this.popup.querySelector<HTMLElement>('.reader-annotation-options')!;
      options.hidden = !options.hidden; button.setAttribute('aria-expanded', String(!options.hidden)); this.scheduleLayout();
    });
    this.popup.querySelectorAll<HTMLButtonElement>('[data-annotation-color]').forEach(button => {
      button.addEventListener('click', () => {
        this.stage({ color: button.dataset.annotationColor! }); this.flush();
        this.popup.querySelector<HTMLElement>('.reader-annotation-icon')!.style.color = button.dataset.annotationColor!;
      });
    });
    this.popup.querySelector('[data-annotation-action=copy]')!.addEventListener('click', () => {
      void navigator.clipboard.writeText(annotation.content).catch(() => this.status('Could not copy text'));
    });
    this.popup.querySelector('[data-annotation-action=delete]')!.addEventListener('click', () => {
      this.flush();
      void this.queue.then(() => this.remove(annotation.id)).catch(() => this.status('Could not delete annotation'));
    });
  }

  private renderTags(tags: string[]) {
    const container = this.popup.querySelector('.reader-annotation-tags')!;
    container.replaceChildren();
    for (const tag of tags) {
      const button = document.createElement('button'); button.type = 'button';
      button.textContent = `${tag} ×`; button.setAttribute('aria-label', `Remove tag ${tag}`);
      button.addEventListener('click', () => {
        const id = this.selected!;
        const current = this.drafts.get(id)?.tags ?? this.annotations.get(id)?.tags ?? tags;
        const next = current.filter(value => value !== tag);
        this.stage({ tags: next }); this.renderTags(next); this.flush();
      });
      container.append(button);
    }
  }

  private stage(patch: AnnotationPatch) {
    if (!this.selected) return;
    this.pending.set(this.selected, { ...this.pending.get(this.selected), ...patch });
    this.drafts.set(this.selected, { ...this.drafts.get(this.selected), ...patch });
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = window.setTimeout(() => { this.timer = null; this.flush(); }, 450);
    this.failed.delete(this.selected);
    this.status('Unsaved changes');
  }

  private flush() {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    for (const [id, patch] of this.pending) {
      this.pending.delete(id);
      this.queue = this.queue.then(async () => {
        if (this.selected === id) this.status('Saving…');
        try {
          const annotation = await this.update(id, patch);
          this.annotations.set(id, annotation);
          this.failed.delete(id);
          const draft = this.drafts.get(id);
          if (draft) {
            for (const key of ['comment', 'tags', 'color'] as const) {
              if (key in patch && JSON.stringify(draft[key]) === JSON.stringify(patch[key])) delete draft[key];
            }
            if (!Object.keys(draft).length) this.drafts.delete(id);
          }
          if (this.selected === id) this.status(this.pending.has(id) ? 'Unsaved changes' : this.drafts.has(id) ? 'Saving…' : 'Saved');
        } catch {
          this.failed.add(id);
          this.pending.set(id, { ...patch, ...this.pending.get(id) });
          if (this.selected === id) this.status('Could not save. ', true);
        }
      });
    }
  }

  private status(message: string, retry = false) {
    const status = this.popup.querySelector('.reader-annotation-save-status');
    if (!status) return;
    status.replaceChildren(document.createTextNode(message));
    if (retry) {
      const button = document.createElement('button'); button.type = 'button'; button.textContent = 'Retry';
      button.addEventListener('click', () => this.flush()); status.append(button);
    }
  }

  private scheduleLayout() {
    if (this.frame || this.disposed) return;
    this.frame = requestAnimationFrame(() => { this.frame = 0; this.layout(); });
  }

  private layout() {
    this.root.querySelectorAll<HTMLElement>('.reader-annotation-list-item').forEach(item => item.setAttribute('aria-pressed', String(item.dataset.annotationId === this.selected)));
    const marks = this.selected ? this.marks(this.selected) : [];
    const rects = marks.map(mark => mark.getBoundingClientRect());
    if (!rects.length) { this.hoverCard.hide(); this.outline.hidden = true; return; }
    const area = marks[0].closest('.reader-page-area,.reader-reflow')!.getBoundingClientRect();
    const left = Math.max(area.left, Math.min(...rects.map(rect => rect.left)) - 3);
    const right = Math.min(area.right, Math.max(...rects.map(rect => rect.right)) + 3);
    const top = Math.max(area.top, Math.min(...rects.map(rect => rect.top)) - 3);
    const bottom = Math.min(area.bottom, Math.max(...rects.map(rect => rect.bottom)) + 3);
    if (bottom <= top || right <= left) { this.hoverCard.hide(); this.outline.hidden = true; return; }
    this.outline.hidden = false;
    Object.assign(this.outline.style, { left: `${left}px`, top: `${top}px`, width: `${right - left}px`, height: `${bottom - top}px` });
    this.hoverCard.show({ left, top, width: right - left, height: bottom - top });
  }

  close() {
    this.commitTagInput?.(); this.commitTagInput = null;
    this.flush(); this.selected = null;
    this.hoverCard.hide(); this.outline.hidden = true;
    this.root.querySelectorAll('.reader-annotation-list-item[aria-pressed=true]').forEach(item => item.setAttribute('aria-pressed', 'false'));
  }

  destroy() {
    this.close(); this.disposed = true; this.events.abort();
    this.mutations.disconnect(); this.resize.disconnect();
    if (this.frame) cancelAnimationFrame(this.frame);
    this.hoverCard.destroy(); this.popup.remove(); this.outline.remove();
  }
}
