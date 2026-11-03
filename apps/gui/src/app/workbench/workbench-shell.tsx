/**
 * Workbench shell: title bar / activity bar / side bar / editor area /
 * bottom panel / status bar — the VSCode layout skeleton. Every window is
 * an editor tab: agent chat portals into the active agent tab's slot,
 * wiki entries, bibliography, config and DAG are regular tabs.
 *
 * Panel-count stability: the center ResizablePanelGroup always mounts its
 * two panels; hiding the bottom panel collapses it to 0% (collapse →
 * expand, never unmount), so editor bodies never churn.
 *
 * Agent tab sync: the runtime's openTabs/activeAgent (persisted view
 * windows) mirror the workbench agent tabs — opening/selecting in one
 * place converges in the other.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { PanelImperativeHandle } from "react-resizable-panels";
import { gatewayRequest } from "@/gateway";
import { AgentWorkspace, AgentDialogs, SpawnAgentDialog } from "@/features/agent";
import { SearchDialog } from "@/features/wiki/search-dialog";
import { closeWikiSearch, useWikiSearchOpen } from "@/features/wiki/wiki-state";
import { useMediaQuery } from "@/hooks/use-media-query";
import { cn } from "@/lib/utils";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { registerCommands } from "./command-registry";
import { CommandPalette } from "./command-palette";
import { registerWorkbenchEditorKinds } from "./editor-kinds";
import { useAgentSlot } from "./agent-slot-registry";
import "./static-commands";
import { useWorkbenchKeybindings } from "./keybindings";
import {
  activateEditorById,
  agentEditorId,
  bumpUnreadNotices,
  closeEditor,
  openAgentEditor,
  openEditor,
  openWikiEntryEditor,
  setPanelHeight,
  setSidebarCollapsed,
  togglePanel,
  toggleSidebar,
  useWorkbenchState,
} from "./workbench-store";
import { openBibliographyEditor } from "./static-commands";
import { WorkbenchRuntimeContext, type AgentRuntime } from "./runtime-context";
import { TitleBar } from "./parts/title-bar";
import { ActivityBar } from "./parts/activity-bar";
import { SideBar } from "./parts/side-bar";
import { EditorArea } from "./parts/editor-area";
import { BottomPanel } from "./parts/bottom-panel";
import { StatusBar } from "./parts/status-bar";

// Editor kinds register once at module load.
registerWorkbenchEditorKinds();

/** Wiki search dialog driven by the wiki search store; results open
 *  wiki-entry editor tabs through the workbench opener. */
function WikiSearchDialog() {
  const open = useWikiSearchOpen();
  return (
    <SearchDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) closeWikiSearch();
      }}
      onOpen={(id, title) => {
        closeWikiSearch();
        openWikiEntryEditor(id, { title });
      }}
    />
  );
}

export function WorkbenchShell({ runtime }: { runtime: AgentRuntime }) {
  const state = useWorkbenchState();
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const mobile = !isDesktop;
  useWorkbenchKeybindings();

  const [spawnOpen, setSpawnOpen] = useState(false);

  // Viewport crossings reset the sidebar: drawer closed on mobile; entering
  // desktop keeps the rail as-is. The first run is skipped so the restored
  // layout stays in charge on initial load.
  const crossedOnceRef = useRef(false);
  useEffect(() => {
    if (!crossedOnceRef.current) {
      crossedOnceRef.current = true;
      return;
    }
    if (!isDesktop) setSidebarCollapsed(true);
  }, [isDesktop]);

  // Mirror the bottom-panel visibility onto its collapsible panel
  // (collapse → 0%, expand — never unmount, so editor bodies keep their
  // DOM and state).
  const bottomPanelRef = useRef<PanelImperativeHandle | null>(null);
  useEffect(() => {
    const panel = bottomPanelRef.current;
    if (!panel) return;
    if (state.panel.visible) {
      if (panel.isCollapsed()) panel.expand();
    } else if (!panel.isCollapsed()) {
      panel.collapse();
    }
  }, [state.panel.visible]);

  // Unread notice badge: count notices that arrived while the notices tab
  // was not in front.
  const seenNoticesRef = useRef(0);
  useEffect(() => {
    const count = runtime.notices.length;
    if (count > seenNoticesRef.current) {
      const hiddenCount = count - seenNoticesRef.current;
      if (!state.panel.visible || state.panel.tab !== "notices") bumpUnreadNotices(hiddenCount);
    }
    seenNoticesRef.current = Math.max(count, seenNoticesRef.current);
  }, [runtime.notices.length, state.panel.visible, state.panel.tab]);

  // Agent tab sync (runtime openTabs/activeAgent ⇔ workbench agent tabs).
  useEffect(() => {
    const openTabs = runtime.openTabs;
    const activeAgent = runtime.activeAgent;
    const livePaths = new Set<string>();
    const activePaths = new Set<string>();
    const main = state.groups.main;
    const secondary = state.groups.secondary;
    for (const [groupId, group] of [["main", main], ["secondary", secondary]] as const) {
      for (const editor of group?.editors ?? []) {
        if (editor.kind !== "agent") continue;
        const path = String(editor.params.path ?? "");
        livePaths.add(path);
        if (editor.id === group?.activeId) activePaths.add(path);
        void groupId;
      }
    }
    // Open tabs the runtime knows but the strip lacks (openTabs order).
    for (const path of openTabs) {
      if (!livePaths.has(path)) openAgentEditor(path, path === activeAgent);
    }
    // Close tabs the runtime dropped (closeTab from the strip already ran).
    for (const path of livePaths) {
      if (!openTabs.includes(path)) closeEditor(agentEditorId(path));
    }
    // Focus follows the runtime's active agent — but only when an agent
    // tab is already the active editor (tab switching mirrors selection)
    // or nothing is active yet. Opening another kind of editor must not
    // have the sync effect steal focus back to the chat.
    if (activeAgent && openTabs.includes(activeAgent) && !activePaths.has(activeAgent)) {
      const activeGroup = state.groups.active === "secondary" ? state.groups.secondary : state.groups.main;
      const activeEditor = activeGroup?.editors.find((editor) => editor.id === activeGroup?.activeId);
      if (!activeEditor || activeEditor.kind === "agent") {
        activateEditorById(agentEditorId(activeAgent));
      }
    }
  }, [runtime.openTabs, runtime.activeAgent, state.groups]);

  // Effort refresh keyed to the active agent (workspace composer needs it).
  const refreshEffort = useCallback(() => {
    void runtime.refreshAgentEffort(runtime.activeAgent);
  }, [runtime.refreshAgentEffort, runtime.activeAgent]);

  const cancel = useCallback(() => {
    void gatewayRequest("POST", `/agents/${runtime.activeAgent}/cancel`).catch((cause) => window.alert(String(cause)));
  }, [runtime.activeAgent]);

  const compact = useCallback(() => {
    void gatewayRequest("POST", `/agents/${runtime.activeAgent}/compact`).catch((cause) => window.alert(String(cause)));
  }, [runtime.activeAgent]);

  const openDag = useCallback(() => {
    if (runtime.activeAgent) openEditor({ kind: "dag", params: { agentPath: runtime.activeAgent } });
  }, [runtime.activeAgent]);

  // Runtime-dependent commands (owner "shell-runtime").
  useEffect(() => registerCommands("shell-runtime", [
    { id: "agent.new", title: "New agent", category: "agent", keywords: "spawn create start profile", run: () => setSpawnOpen(true) },
    { id: "agent.cancel", title: "Cancel agent", category: "agent", keywords: "stop abort interrupt", run: cancel },
    { id: "agent.compact", title: "Compact conversation", category: "agent", keywords: "compress summarize context", run: compact },
    { id: "agent.model", title: "Select model", category: "config", keywords: "model provider api key activate", run: () => openEditor({ kind: "config" }) },
    { id: "agent.dag", title: "View DAG", category: "data", keywords: "graph pipeline topology node edge", run: openDag },
    { id: "agent.open", title: "Open agent tab", category: "agent", keywords: "open chat transcript window", run: () => openAgentEditor(runtime.activeAgent) },
    { id: "view.thinking", title: "Toggle thinking blocks", category: "view", keywords: "collapse fold reasoning", run: () => void runtime.toggleDisplay("collapse_thinking") },
    { id: "view.tool-calls", title: "Toggle tool calls", category: "view", keywords: "collapse fold function call", run: () => void runtime.toggleDisplay("collapse_tool_calls") },
    { id: "view.tool-results", title: "Toggle tool results", category: "view", keywords: "collapse fold output", run: () => void runtime.toggleDisplay("collapse_tool_results") },
    { id: "go.library", title: "Bibliography library", category: "research", keywords: "papers articles fulltext collections", run: () => openBibliographyEditor("library") },
    { id: "go.discover", title: "Discover literature", category: "research", keywords: "federated search sources subscriptions", run: () => openBibliographyEditor("discover") },
    { id: "go.snapshots", title: "Search snapshots", category: "research", keywords: "snapshots history", run: () => openBibliographyEditor("snapshots") },
    { id: "go.subscriptions", title: "Subscriptions", category: "research", keywords: "feeds inbox", run: () => openBibliographyEditor("subscriptions") },
  ]), [runtime, cancel, compact, openDag]);

  const logout = useCallback(() => void runtime.logout(), [runtime]);

  const handleCenterLayout = useCallback((layout: Record<string, number>) => {
    const size = layout["workbench-panel"];
    if (state.panel.visible && size >= 15 && size <= 70) setPanelHeight(size);
  }, [state.panel.visible]);

  const activeAgent = runtime.activeAgent;
  const activeAgentSlot = useAgentSlot(activeAgent);

  return (
    <WorkbenchRuntimeContext.Provider value={runtime}>
      <main
        data-workbench=""
        data-sidebar-collapsed={state.sidebar.collapsed || undefined}
        className="relative grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)_auto] bg-background text-foreground"
      >
        <TitleBar
          mobile={mobile}
          sidebarCollapsed={state.sidebar.collapsed}
          onToggleSidebar={toggleSidebar}
          panelVisible={state.panel.visible}
          onTogglePanel={togglePanel}
          onLogout={logout}
        />

        <div className="flex min-h-0 min-w-0">
          <ActivityBar
            mobile={mobile}
            onToggleSidebar={toggleSidebar}
            onTogglePanel={togglePanel}
            onLogout={logout}
          />
          {!mobile && !state.sidebar.collapsed && (
            <SideBar mobile={false} collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} />
          )}
          <ResizablePanelGroup orientation="vertical" onLayoutChanged={handleCenterLayout} className="min-h-0 min-w-0 flex-1">
            <ResizablePanel id="workbench-editors" minSize="30%" className="flex min-w-0">
              <EditorArea />
            </ResizablePanel>
            <ResizableHandle
              aria-label="调整面板高度"
              className={cn(!state.panel.visible && "pointer-events-none w-0 opacity-0")}
            />
            <ResizablePanel
              id="workbench-panel"
              panelRef={bottomPanelRef}
              defaultSize={`${state.panel.heightPercent}%`}
              minSize="80px"
              maxSize="70%"
              collapsible
              collapsedSize="0%"
              className="flex min-h-0 min-w-0 flex-col"
            >
              <BottomPanel frameLog={runtime.frameLog} notices={runtime.notices} />
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>

        <StatusBar runtime={runtime} />

        {mobile && (
          <SideBar mobile collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} />
        )}

        {/* One workspace instance portals into the active agent tab's slot
            (agent-slot-registry); it parks off-document when no agent tab
            is active. */}
        <AgentWorkspace
          dockedSlot={activeAgentSlot}
          agents={runtime.agents}
          activeAgent={activeAgent}
          statuses={runtime.statuses}
          messages={runtime.messages[activeAgent] ?? []}
          historyLoading={runtime.historyLoading[activeAgent] ?? false}
          streaming={runtime.streaming[activeAgent] ?? null}
          usage={runtime.usage[activeAgent] ?? null}
          display={runtime.display}
          draft={runtime.draft}
          busy={runtime.busy}
          onDraftChange={runtime.setDraft}
          onSendMessage={(context) => void runtime.sendMessage(context)}
          onCancel={cancel}
          onCompact={compact}
          modelCatalog={runtime.modelCatalog}
          onLoadModelCatalog={runtime.loadModelCatalog}
          onChangeModel={(path, spec) => runtime.setAgentModel(path, spec)}
          agentEffort={runtime.agentEfforts[activeAgent]}
          onRefreshEffort={refreshEffort}
          onChangeEffort={(path, effort) => runtime.setAgentEffort(path, effort)}
          onSelectAgent={(path) => runtime.selectAgent(path)}
          openTabs={runtime.openTabs}
          onCloseAgent={runtime.closeTab}
          onSpawn={() => setSpawnOpen(true)}
          storedAgents={runtime.storedAgents}
          onSearchStoredAgents={(query) => void runtime.refreshStoredAgents(query)}
          onResumeAgent={(agent) => void runtime.resumeAgent(agent)}
          onOpenDag={openDag}
        />

        <AgentDialogs
          activeAgent={activeAgent}
          agents={runtime.agents}
          messages={runtime.messages[activeAgent] ?? []}
          promptHistory={runtime.promptHistories[activeAgent] ?? []}
          onDraftChange={runtime.setDraft}
          onResumeAgent={runtime.resumeAgent}
          onShutdownAgent={runtime.shutdownAgent}
          onCloseTab={runtime.closeTab}
        />

        <SpawnAgentDialog
          open={spawnOpen}
          onOpenChange={setSpawnOpen}
          busy={runtime.busy}
          onSpawnAgent={runtime.spawnAgent}
        />

        <WikiSearchDialog />

        <CommandPalette />
      </main>
    </WorkbenchRuntimeContext.Provider>
  );
}
