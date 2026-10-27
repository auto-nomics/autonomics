// Test-only mount helper: served by Vite so bare 'react' imports resolve.
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { BibliographyView } from "@/features/bibliography";

export function mountBibliography(target: HTMLElement) {
  target.style.height = "100vh"; target.style.display = "flex"; target.style.flexDirection = "column";
  createRoot(target).render(createElement(BibliographyView));
}
