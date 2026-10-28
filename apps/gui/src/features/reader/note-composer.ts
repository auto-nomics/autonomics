/** Draft notes before persisting them; closing or cancelling never writes an
 * empty annotation. The editor is shared by point notes and selected-text notes. */
export class NoteComposer {
  private popup = document.createElement('form');
  private disposed = false;
  private saving = false;

  constructor(private root: HTMLElement) {
    this.popup.className = 'reader-note-composer';
    this.popup.hidden = true;
    this.popup.setAttribute('role', 'dialog');
    this.popup.setAttribute('aria-label', 'Add note');
    root.append(this.popup);
  }

  open(point: { x: number; y: number }, save: (text: string) => Promise<boolean>) {
    if (this.saving) return;
    this.popup.innerHTML = `<label>New note<textarea aria-label="Note text" placeholder="Write a note…" rows="4" required></textarea></label><div class="reader-note-actions"><button type="button" data-action="cancel-note">Cancel</button><button type="submit" disabled>Save note</button></div>`;
    this.popup.hidden = false;
    this.popup.style.left = `${Math.max(8, Math.min(point.x, window.innerWidth - 300))}px`;
    this.popup.style.top = `${Math.max(8, Math.min(point.y + 12, window.innerHeight - 240))}px`;
    const input = this.popup.querySelector('textarea')!;
    const button = this.popup.querySelector<HTMLButtonElement>('[type=submit]')!;
    input.addEventListener('input', () => { button.disabled = !input.value.trim(); });
    this.popup.querySelector('[data-action=cancel-note]')!.addEventListener('click', () => this.close());
    this.popup.onsubmit = async event => {
      event.preventDefault();
      const text = input.value.trim();
      if (!text || this.saving) return;
      this.saving = true; input.disabled = true; button.disabled = true; button.textContent = 'Saving…';
      const saved = await save(text);
      this.saving = false;
      if (this.disposed) return;
      input.disabled = false; button.disabled = false; button.textContent = 'Save note';
      if (saved) this.close();
      else input.focus();
    };
    this.popup.onkeydown = event => {
      if (event.key === 'Escape') { event.stopPropagation(); this.close(); }
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
        event.preventDefault(); this.popup.requestSubmit();
      }
    };
    input.focus();
  }

  close() {
    if (this.saving) return;
    this.popup.hidden = true;
    // Return focus only when focus was inside the editor.
    if (this.popup.contains(document.activeElement)) this.root.querySelector<HTMLElement>('.reader-page-area')?.focus();
  }

  destroy() { this.disposed = true; this.popup.remove(); }
}
