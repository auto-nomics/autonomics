// Test-only mount helper: served by Vite so bare 'react' imports resolve.
// In the app the workspace tabs live in the sidebar; standalone harnesses
// recreate that tab strip here so workflow tests can switch workspaces.
import { createElement, useState } from "react";
import { createRoot } from "react-dom/client";
import { BibliographyView, type BibliographyWorkspace } from "@/features/bibliography";

const workspaces: Array<[BibliographyWorkspace, string]> = [
  ["library", "Library"],
  ["discover", "Discover"],
  ["snapshots", "Snapshots"],
  ["subscriptions", "Subscriptions"],
];

export function mountBibliography(target: HTMLElement) {
  target.style.height = "100vh"; target.style.display = "flex"; target.style.flexDirection = "column";
  function Harness() {
    const [workspace, setWorkspace] = useState<BibliographyWorkspace>("library");
    return createElement("div", { style: { display: "flex", minHeight: 0, flex: 1, flexDirection: "column" } },
      createElement("nav", { "aria-label": "Bibliography workspace", style: { display: "flex", flexWrap: "wrap", gap: 2, padding: 4, borderBottom: "1px solid #ddd" } },
        workspaces.map(([id, label]) => createElement("button", { key: id, type: "button", "aria-current": workspace === id ? "page" : undefined, onClick: () => setWorkspace(id) }, label)),
      ),
      createElement(BibliographyView, { workspace }),
    );
  }
  createRoot(target).render(createElement(Harness));
}
