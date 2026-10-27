import './alert-dialog.css';

export interface AlertDialogOptions {
  title: string;
  description?: string;
  actionLabel?: string;
  cancelLabel?: string;
  /** Render the action as the destructive (red) variant with a warning icon. */
  destructive?: boolean;
}

const ALERT_ICON = '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4M12 17h.01"/>';

let serial = 0;

/** A modal confirmation dialog for imperative views, following the shadcn
 * alert-dialog anatomy (overlay, title, description, cancel + action).
 * `open()` resolves true when the action is chosen and false when the dialog
 * is dismissed. Mount it with the owning view so pending promises, focus and
 * listeners are cleaned up when that view closes. */
export class AlertDialog {
  private readonly region = document.createElement('div');
  private pending: ((confirmed: boolean) => void) | null = null;
  private restoreFocus: HTMLElement | null = null;

  constructor(container: HTMLElement) {
    this.region.className = 'app-alert-overlay';
    this.region.hidden = true;
    container.append(this.region);
  }

  open(options: AlertDialogOptions): Promise<boolean> {
    this.close(false);
    this.restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const id = `app-alert-${++serial}`;
    const dialog = document.createElement('div');
    dialog.className = 'app-alert-dialog';
    dialog.setAttribute('role', 'alertdialog');
    dialog.setAttribute('aria-modal', 'true');
    dialog.setAttribute('aria-labelledby', `${id}-title`);
    if (options.destructive) {
      const icon = document.createElement('div');
      icon.className = 'app-alert-icon';
      icon.setAttribute('aria-hidden', 'true');
      icon.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ALERT_ICON}</svg>`;
      dialog.append(icon);
    }
    const title = document.createElement('h2');
    title.id = `${id}-title`;
    title.className = 'app-alert-title';
    title.textContent = options.title;
    dialog.append(title);
    if (options.description) {
      dialog.setAttribute('aria-describedby', `${id}-description`);
      const description = document.createElement('p');
      description.id = `${id}-description`;
      description.className = 'app-alert-description';
      description.textContent = options.description;
      dialog.append(description);
    }
    const actions = document.createElement('div');
    actions.className = 'app-alert-actions';
    const cancel = this.button(options.cancelLabel ?? 'Cancel', 'outline', () => this.close(false));
    const action = this.button(options.actionLabel ?? 'OK', options.destructive ? 'destructive' : 'primary', () => this.close(true));
    actions.append(cancel, action);
    dialog.append(actions);
    dialog.addEventListener('keydown', event => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        this.close(false);
      } else if (event.key === 'Tab') {
        event.preventDefault();
        const order = [cancel, action];
        const current = order.indexOf(document.activeElement as HTMLButtonElement);
        const next = event.shiftKey
          ? order[(current - 1 + order.length) % order.length]
          : order[(current + 1) % order.length];
        next.focus();
      }
    });
    this.region.replaceChildren(dialog);
    this.region.hidden = false;
    this.region.addEventListener('mousedown', this.handleBackdrop);
    cancel.focus();
    return new Promise(resolve => { this.pending = resolve; });
  }

  destroy() {
    this.close(false);
    this.region.remove();
  }

  private button(label: string, variant: 'outline' | 'primary' | 'destructive', onClick: () => void) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'app-alert-button';
    button.dataset.variant = variant;
    button.textContent = label;
    button.addEventListener('click', onClick);
    return button;
  }

  private close(confirmed: boolean) {
    if (this.region.hidden) return;
    this.region.hidden = true;
    this.region.replaceChildren();
    this.region.removeEventListener('mousedown', this.handleBackdrop);
    const resolve = this.pending;
    this.pending = null;
    resolve?.(confirmed);
    if (this.restoreFocus?.isConnected) this.restoreFocus.focus();
    this.restoreFocus = null;
  }

  private readonly handleBackdrop = (event: MouseEvent) => {
    if (event.target === this.region) this.close(false);
  };
}
