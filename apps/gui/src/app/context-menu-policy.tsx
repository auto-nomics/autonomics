import { useEffect } from "react";

/**
 * Owns the WebView's fallback context-menu behaviour. Feature menus register
 * themselves through ContextMenuTrigger; editable controls and text selections
 * intentionally retain the platform menu (paste, spellcheck, copy, etc.).
 *
 * Use `data-context-menu="native"` for an exceptional non-editable region
 * that must retain its browser menu.
 */
export function ContextMenuPolicy() {
  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Element)) return;

      // Registered application menus take precedence over browser behaviour.
      if (target.closest('[data-context-menu="custom"]')) {
        event.preventDefault();
        return;
      }

      if (target.closest('[data-context-menu="native"]')) return;

      // Keep the OS/WebView menu where it supplies expected editing commands.
      if (target.closest('input, textarea, select, [contenteditable="true"]')) return;

      // Reading views retain Copy and other selection-specific browser actions
      // until they provide their own registered selection menu.
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed && selection.toString().trim()) return;

      // The rest of the application has no implicit browser context menu.
      event.preventDefault();
    };

    document.addEventListener("contextmenu", onContextMenu, { capture: true });
    return () => document.removeEventListener("contextmenu", onContextMenu, { capture: true });
  }, []);

  return null;
}
