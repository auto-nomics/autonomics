/**
 * Global workbench keybindings. Installed once by the shell; agent-scoped
 * keys (Ctrl+R prompt history) stay with the agent dialogs component.
 * Browser-reserved combinations (Ctrl+T/W/N…) are deliberately untouched.
 */

import { useEffect } from "react";
import { openWikiSearch } from "@/features/wiki/wiki-state";
import { openPalette } from "./command-registry";
import { togglePanel, toggleSidebar } from "./workbench-store";

export function useWorkbenchKeybindings(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const mod = event.ctrlKey || event.metaKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      if (key === "p") {
        // Shift turns quick-open into the commands-only palette.
        event.preventDefault();
        openPalette(event.shiftKey ? "commands" : "all");
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
