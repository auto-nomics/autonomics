/**
 * Static shell commands: depend only on the workbench store and the theme
 * module, so they register once at module load. Runtime-dependent commands
 * are registered by the shell (owner "shell-runtime") instead.
 */

import { cycleThemeMode } from "@/lib/theme";
import { registerCommands } from "./command-registry";
import {
  closeEditor,
  closeSplit,
  getWorkbenchState,
  openAgentEditor,
  openEditor,
  setContainer,
  setContainerView,
  setPanelTab,
  splitEditor,
  togglePanel,
  toggleSidebar,
} from "./workbench-store";

registerCommands("shell", [
  { id: "workbench.toggle-sidebar", title: "Toggle side bar", category: "view", keywords: "ctrl b sidebar visibility", run: toggleSidebar },
  { id: "workbench.toggle-panel", title: "Toggle bottom panel", category: "view", keywords: "ctrl j output events notices", run: togglePanel },
  { id: "workbench.panel-events", title: "Show event stream panel", category: "view", keywords: "events log frames", run: () => { setPanelTab("events"); } },
  { id: "workbench.panel-notices", title: "Show notices panel", category: "view", keywords: "notifications notices", run: () => { setPanelTab("notices"); } },
  { id: "workbench.split-editor", title: "Split editor", category: "view", keywords: "split right group", run: splitEditor },
  { id: "workbench.close-split", title: "Close editor split", category: "view", keywords: "close split group", run: closeSplit },
  // Palette close is a plain close; tab-strip close enforces confirmLeave.
  { id: "workbench.close-editor", title: "Close editor", category: "view", keywords: "close tab", run: () => {
    const current = getWorkbenchState();
    const group = current.groups.active === "secondary" ? current.groups.secondary : current.groups.main;
    const id = group?.activeId;
    if (id) closeEditor(id);
  } },
  { id: "workbench.open-bibliography", title: "Open bibliography", category: "go", keywords: "papers library collections literature", run: openBibliographyEditor },
  { id: "workbench.open-wiki", title: "Open knowledge base", category: "go", keywords: "wiki knowledge entries", run: () => {
    setContainer("wiki");
    setContainerView("wiki", "browse");
  } },
  { id: "workbench.open-wiki-graph", title: "Open knowledge graph", category: "go", keywords: "wiki graph 知识图谱 关系", run: () => openEditor({ kind: "wiki-graph" }) },
  { id: "workbench.open-agents", title: "Show agent overview", category: "go", keywords: "agents overview chat", run: () => {
    setContainer("agents");
    setContainerView("agents", "overview");
  } },
  { id: "workbench.open-config", title: "Open settings", category: "go", keywords: "config model provider preferences", run: () => openEditor({ kind: "config" }) },
  { id: "workbench.cycle-theme", title: "Cycle color theme", category: "view", keywords: "light dark system theme", run: () => { cycleThemeMode(); } },
]);

/** Reveals the bibliography container and opens (or refocuses) its editor
 *  on the given workspace view. Used by the sidebar views and commands. */
export function openBibliographyEditor(workspace = "library"): void {
  if (workspace === "library") {
    setContainer("bibliography");
    setContainerView("bibliography", "library");
  }
  openEditor({ kind: "bibliography", params: { workspace } });
}

/** Opens (or focuses) the agent's editor tab — the unified chat window. */
export function openAgentTab(path: string): void {
  if (path) openAgentEditor(path);
}
