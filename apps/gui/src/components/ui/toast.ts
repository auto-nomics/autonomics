import './toast.css';

type ToastKind = 'success' | 'error' | 'info';

const ICONS: Record<ToastKind, string> = {
  success: '<circle cx="12" cy="12" r="9"/><path d="m8 12 3 3 5-6"/>',
  error: '<circle cx="12" cy="12" r="9"/><path d="M12 7v6M12 17h.01"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>',
};

/** A small toast host for imperative views. Mount it with the owning view so
 * notifications and their timers are cleaned up when that view closes. */
export class Toast {
  private readonly region = document.createElement('div');
  private timer: number | null = null;

  constructor(container: HTMLElement) {
    this.region.className = 'app-toast-viewport';
    this.region.setAttribute('role', 'status');
    this.region.setAttribute('aria-live', 'polite');
    this.region.setAttribute('aria-atomic', 'true');
    container.append(this.region);
  }

  show(message: string, kind: ToastKind = 'success') {
    this.dismiss();
    const toast = document.createElement('div');
    toast.className = 'app-toast';
    toast.dataset.kind = kind;
    const icon = document.createElement('span');
    icon.className = 'app-toast-icon';
    icon.setAttribute('aria-hidden', 'true');
    icon.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[kind]}</svg>`;
    const text = document.createElement('span');
    text.className = 'app-toast-message';
    text.textContent = message;
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'app-toast-close';
    close.setAttribute('aria-label', 'Dismiss notification');
    close.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="m6 6 12 12M6 18 18 6"/></svg>';
    close.addEventListener('click', () => this.dismiss());
    toast.append(icon, text, close);
    this.region.append(toast);
    this.timer = window.setTimeout(() => this.dismiss(), kind === 'error' ? 4500 : 2500);
  }

  dismiss() {
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
    this.region.replaceChildren();
  }

  destroy() {
    this.dismiss();
    this.region.remove();
  }
}
