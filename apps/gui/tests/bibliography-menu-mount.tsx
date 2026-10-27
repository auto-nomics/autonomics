// Test-only mount helper: served by Vite so bare 'react' imports resolve.
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { BibliographyView } from "@/features/bibliography";

export function mountBibliography(target: HTMLElement) {
  createRoot(target).render(createElement(BibliographyView));
}
