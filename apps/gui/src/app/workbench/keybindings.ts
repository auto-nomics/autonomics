/**
 * Global workbench keybindings. Installed once by the shell; agent-scoped
 * keys (Ctrl+R prompt history) stay with the agent dialogs component.
 * Browser-reserved combinations (Ctrl+T/W/N…) are deliberately untouched.
 */

import { useEffect } from "react";
import { openWikiSearch } from "@/features/wiki/wiki-state";
import { openPalette } from "./command-registry";
import { togglePanel, toggleSidebar } from "./workbench-store";
import { toggleSecondarySidebar } from "./view-layout";

/** Cycle workbench regions independently of each region's internal Tab order. */
function cycleWorkbenchFocus(backward: boolean) {
  const regions = Array.from(document.querySelectorAll<HTMLElement>('aside[data-slot="workbench-sidebar"], [data-editor-group], section[aria-label="底部面板"], aside[aria-label="辅助侧边栏"]'))
    .filter(element => element.getClientRects().length > 0 && element.getBoundingClientRect().width > 0 && element.getBoundingClientRect().height > 0);
  if (!regions.length) return;
  const current = regions.findIndex(region => region.contains(document.activeElement));
  const next = regions[(current + (backward ? -1 : 1) + regions.length) % regions.length];
  const focusable = Array.from(next.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex="0"]'))
    .find(element => element.getClientRects().length > 0 && !element.closest('[inert]'));
  if (focusable) focusable.focus();
  else { next.tabIndex = -1; next.focus(); }
}

export function useWorkbenchKeybindings(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "F6" && !event.ctrlKey && !event.metaKey && !event.altKey) {
        if (document.activeElement?.closest('[role="dialog"]')) return;
        event.preventDefault();
        cycleWorkbenchFocus(event.shiftKey);
        return;
      }
      const mod = event.ctrlKey || event.metaKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      if (key === "p") {
        // Shift turns quick-open into the commands-only palette.
        event.preventDefault();
        openPalette(event.shiftKey ? "commands" : "all");
      } else if (key === "b" && !event.shiftKey && event.altKey) {
        event.preventDefault();
        toggleSecondarySidebar();
      } else if (key === "b" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        toggleSidebar();
      } else if (key === "j" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        togglePanel();
      } else if (key === "k" && !event.shiftKey && !event.altKey) {
        // Wiki quick search — global since the wiki panel is tabs now.
        event.preventDefault();
        openWikiSearch();
      }
      // Ctrl+R stays with the prompt-history dialog.
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
