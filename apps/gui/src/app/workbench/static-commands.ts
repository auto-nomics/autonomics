import i18n from "@/i18n";
import { resetWorkbenchViewLocations, toggleSecondarySidebar } from "./view-layout";
/**
 * Static shell commands: depend only on the workbench store and the theme
 * module, so they register once at module load. Runtime-dependent commands
 * are registered by the shell (owner "shell-runtime") instead.
 */

import { openWikiSearch } from "@/features/wiki/wiki-state";
import { cycleThemeMode } from "@/lib/theme";
import { openPalette, registerCommands } from "./command-registry";
import {
  activateEditorById,
  allGroups,
  closeEditor,
  closeSplit,
  getWorkbenchState,
  getGroup,
  openAgentEditor,
  openEditor,
  patchEditorParams,
  setContainer,
  setContainerView,
  setPanelTab,
  setSidebarCollapsed,
  splitEditor,
  togglePanel,
  toggleSidebar,
} from "./workbench-store";

function registerShellCommands(): void {
registerCommands("shell", [
  { id: "workbench.quick-open", title: i18n.t("workbench:commands.quickOpen"), category: "view", keywords: "quick open switcher palette 快速切换 命令面板", shortcut: "Ctrl + P", run: () => openPalette("all") },
  { id: "workbench.open-command-palette", title: i18n.t("workbench:commands.openCommandPalette"), category: "view", keywords: "命令面板", shortcut: "Ctrl + Shift + P", run: () => openPalette("commands") },
  { id: "workbench.wiki-search", title: i18n.t("workbench:commands.wikiSearch"), category: "go", keywords: "search find fulltext wiki 检索 搜索 全文", shortcut: "Ctrl + K", run: openWikiSearch },
  { id: "workbench.toggle-secondary-sidebar", title: i18n.t("workbench:commands.toggleSecondarySidebar"), category: "view", keywords: "secondary sidebar visibility", shortcut: "Ctrl + Alt + B", run: toggleSecondarySidebar },
  { id: "workbench.reset-view-locations", title: i18n.t("workbench:commands.resetViewLocations"), category: "view", keywords: "reset sidebar panel layout", run: () => resetWorkbenchViewLocations() },
  { id: "workbench.toggle-sidebar", title: i18n.t("workbench:commands.toggleSidebar"), category: "view", keywords: "ctrl b sidebar visibility", shortcut: "Ctrl + B", run: toggleSidebar },
  { id: "workbench.toggle-panel", title: i18n.t("workbench:commands.togglePanel"), category: "view", keywords: "ctrl j output events notices", shortcut: "Ctrl + J", run: togglePanel },
  { id: "workbench.panel-events", title: i18n.t("workbench:commands.panelEvents"), category: "view", keywords: "events log frames", run: () => { setPanelTab("events"); } },
  { id: "workbench.panel-notices", title: i18n.t("workbench:commands.panelNotices"), category: "view", keywords: "notifications notices", run: () => { setPanelTab("notices"); } },
  { id: "workbench.split-editor", title: i18n.t("workbench:commands.splitEditor"), category: "view", keywords: "split right group", run: () => splitEditor() },
  { id: "workbench.close-split", title: i18n.t("workbench:commands.closeSplit"), category: "view", keywords: "close split group", run: () => closeSplit() },
  // Every close path goes through the store's leave guard.
  { id: "workbench.close-editor", title: i18n.t("workbench:commands.closeEditor"), category: "view", keywords: "close tab", run: () => {
    const current = getWorkbenchState();
    const group = getGroup(current.groups, current.groups.active);
    const id = group?.activeId;
    if (id) closeEditor(id);
  } },
  { id: "workbench.open-bibliography", title: i18n.t("workbench:commands.openBibliography"), category: "go", keywords: "papers library collections literature", run: openBibliographyEditor },
  { id: "workbench.open-wiki", title: i18n.t("workbench:commands.openWiki"), category: "go", keywords: "wiki knowledge entries", run: () => {
    setContainer("wiki");
    setContainerView("wiki", "browse");
  } },
  { id: "workbench.open-decks", title: i18n.t("workbench:commands.openDecks"), category: "go", keywords: "deck slides ppt presentation 幻灯片 演示", run: () => {
    setContainer("decks");
  } },
  { id: "workbench.open-wiki-graph", title: i18n.t("workbench:commands.openWikiGraph"), category: "go", keywords: "wiki graph 知识图谱 关系", run: () => { setContainerView("wiki", "graph"); setContainer("wiki"); setSidebarCollapsed(false); } },
  { id: "workbench.open-agents", title: i18n.t("workbench:commands.openAgents"), category: "go", keywords: "agents overview chat", run: () => {
    setContainer("agents");
    setContainerView("agents", "overview");
  } },
  { id: "workbench.open-config", title: i18n.t("workbench:commands.openConfig"), category: "go", keywords: "config model provider preferences", run: () => openEditor({ kind: "config" }) },
  { id: "workbench.open-hpc-jobs", title: i18n.t("workbench:commands.openHpcJobs"), category: "go", keywords: "hpc slurm jobs monitors queue 任务", run: () => openEditor({ kind: "hpc-jobs" }) },
  { id: "workbench.open-vfs", title: i18n.t("workbench:commands.openVfs"), category: "go", keywords: "vfs files browse mounts upload downloads 文件 浏览", run: () => {
    setContainer("vfs");
    setContainerView("vfs", "tree");
    setSidebarCollapsed(false);
  } },
  { id: "workbench.cycle-theme", title: i18n.t("workbench:commands.cycleTheme"), category: "view", keywords: "light dark system theme", run: () => { cycleThemeMode(); } },
]);
}

registerShellCommands();
// Re-register on language change so palette titles follow the active locale.
i18n.on("languageChanged", registerShellCommands);

/** Reveals the bibliography container and opens (or refocuses) its editor
 *  on the given workspace view. Used by the sidebar views and commands. */
export function openBibliographyEditor(workspace = "library"): void {
  if (workspace === "library") {
    setContainer("bibliography");
    setContainerView("bibliography", "library");
  }
  openEditor({ kind: "bibliography", params: { workspace } });
}

/** Reveals the bibliography library and selects the given record's entry.
 *  Reuses the existing library tab instead of spawning one per article.
 *  A per-invocation nonce makes repeat clicks re-run the focus. */
let bibFocusSeq = 0;
export function openBibliographyRecordEditor(articleId: string): void {
  setContainer("bibliography");
  setContainerView("bibliography", "library");
  const focusNonce = ++bibFocusSeq;
  const existing = allGroups(getWorkbenchState().groups)
    .flatMap(([, group]) => group.editors)
    .find(editor => editor.kind === "bibliography" && String(editor.params.workspace ?? "library") === "library");
  if (existing) {
    patchEditorParams(existing.id, { workspace: "library", articleId, focusNonce });
    activateEditorById(existing.id);
  } else {
    openEditor({ kind: "bibliography", params: { workspace: "library", articleId, focusNonce } });
  }
}

/** Opens (or focuses) the agent's editor tab — the unified chat window. */
export function openAgentTab(path: string): void {
  if (path) openAgentEditor(path);
}
