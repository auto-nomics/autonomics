/**
 * Workbench shell: title bar / activity bar / side bar / editor area /
 * bottom panel / auxiliary (agent chat) bar / status bar — the VSCode
 * layout skeleton, hosting the existing feature components.
 *
 * Panel-count stability: both ResizablePanelGroups always mount their two
 * panels; hiding the bottom panel or the aux bar collapses the panel to 0%
 * (collapse → expand, never unmount), so the editor bodies and the
 * portaled AgentWorkspace never churn.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { PanelImperativeHandle } from "react-resizable-panels";
import { gatewayRequest } from "@/gateway";
import { AgentWorkspace, AgentDialogs, SpawnAgentDialog, openAgentDialog } from "@/features/agent";
import { useMediaQuery } from "@/hooks/use-media-query";
import { cn } from "@/lib/utils";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { registerCommands } from "./command-registry";
import { CommandPalette } from "./command-palette";
import { registerWorkbenchEditorKinds } from "./editor-kinds";
import "./static-commands";
import { useWorkbenchKeybindings } from "./keybindings";
import {
  bumpUnreadNotices,
  openEditor,
  setAuxInspectorOpen,
  setAuxWidth,
  setPanelHeight,
  setSidebarCollapsed,
  toggleAux,
  toggleAuxInspector,
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
import { AuxAgentPane } from "./parts/aux-agent-pane";

// Editor kinds register once at module load.
registerWorkbenchEditorKinds();

export function WorkbenchShell({ runtime }: { runtime: AgentRuntime }) {
  const state = useWorkbenchState();
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const mobile = !isDesktop;
  useWorkbenchKeybindings();

  const [auxSlot, setAuxSlot] = useState<HTMLDivElement | null>(null);
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

  // Mirror visibility flags onto the collapsible panels (collapse → 0%,
  // expand — never unmount, so portal targets stay connected).
  const auxPanelRef = useRef<PanelImperativeHandle | null>(null);
  useEffect(() => {
    const panel = auxPanelRef.current;
    if (!panel) return;
    if (state.aux.visible) {
      if (panel.isCollapsed()) panel.expand();
    } else if (!panel.isCollapsed()) {
      panel.collapse();
    }
  }, [state.aux.visible]);

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
    { id: "view.thinking", title: "Toggle thinking blocks", category: "view", keywords: "collapse fold reasoning", run: () => void runtime.toggleDisplay("collapse_thinking") },
    { id: "view.tool-calls", title: "Toggle tool calls", category: "view", keywords: "collapse fold function call", run: () => void runtime.toggleDisplay("collapse_tool_calls") },
    { id: "view.tool-results", title: "Toggle tool results", category: "view", keywords: "collapse fold output", run: () => void runtime.toggleDisplay("collapse_tool_results") },
    { id: "go.library", title: "Bibliography library", category: "research", keywords: "papers articles fulltext collections", run: () => openBibliographyEditor("library") },
    { id: "go.discover", title: "Discover literature", category: "research", keywords: "federated search sources subscriptions", run: () => openBibliographyEditor("discover") },
    { id: "go.snapshots", title: "Search snapshots", category: "research", keywords: "snapshots history", run: () => openBibliographyEditor("snapshots") },
    { id: "go.subscriptions", title: "Subscriptions", category: "research", keywords: "feeds inbox", run: () => openBibliographyEditor("subscriptions") },
  ]), [runtime, cancel, compact, openDag]);

  const logout = useCallback(() => void runtime.logout(), [runtime]);

  const handleBandLayout = useCallback((layout: Record<string, number>) => {
    const size = layout["agent-assistant"];
    if (state.aux.visible && size >= 20 && size <= 60) setAuxWidth(size);
  }, [state.aux.visible]);

  const handleCenterLayout = useCallback((layout: Record<string, number>) => {
    const size = layout["workbench-panel"];
    if (state.panel.visible && size >= 15 && size <= 70) setPanelHeight(size);
  }, [state.panel.visible]);

  const activeAgent = runtime.activeAgent;
  const activeInfo = runtime.agents.find((agent) => agent.path === activeAgent);

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
          auxVisible={state.aux.visible}
          onToggleAux={toggleAux}
          onLogout={logout}
        />

        {mobile ? (
          <div className="flex min-h-0 min-w-0 flex-col">
            <EditorArea />
          </div>
        ) : (
          <div className="flex min-h-0 min-w-0">
            <ActivityBar
              mobile={false}
              onToggleSidebar={toggleSidebar}
              onToggleAux={toggleAux}
              onTogglePanel={togglePanel}
              onLogout={logout}
            />
            {!state.sidebar.collapsed && (
              <SideBar mobile={false} collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} />
            )}
            <ResizablePanelGroup
              orientation="horizontal"
              onLayoutChanged={handleBandLayout}
              className="min-h-0 min-w-0 flex-1"
            >
              <ResizablePanel id="workbench-editor" minSize="35%" className="flex min-w-0 flex-col">
                <ResizablePanelGroup
                  orientation="vertical"
                  onLayoutChanged={handleCenterLayout}
                  className="min-h-0 min-w-0 flex-1"
                >
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
              </ResizablePanel>
              <ResizableHandle
                aria-label="调整 Agent 辅助区宽度"
                className={cn(!state.aux.visible && "pointer-events-none w-0 opacity-0")}
              />
              <ResizablePanel
                id="agent-assistant"
                panelRef={auxPanelRef}
                defaultSize={`${state.aux.widthPercent}%`}
                minSize="320px"
                maxSize="60%"
                collapsible
                collapsedSize="0%"
                className="flex min-w-0 flex-col"
              >
                <aside aria-label="Agent 辅助区" className="flex min-h-0 min-w-0 flex-1 flex-col">
                  <AuxAgentPane
                    onSlotChange={setAuxSlot}
                    inspectorOpen={state.aux.inspectorOpen}
                    onSetInspectorOpen={setAuxInspectorOpen}
                    activeAgent={activeAgent}
                    activeInfo={activeInfo}
                    status={runtime.statuses[activeAgent]}
                    plan={runtime.plans[activeAgent] ?? null}
                    usage={runtime.usage[activeAgent] ?? null}
                    display={runtime.display}
                    onToggleDisplay={(key) => void runtime.toggleDisplay(key)}
                    onOpenDag={openDag}
                    onOpenCopy={() => openAgentDialog("copy")}
                    onOpenHistory={() => openAgentDialog("history")}
                    onRefreshHistory={runtime.refreshHistory}
                  />
                </aside>
              </ResizablePanel>
            </ResizablePanelGroup>
          </div>
        )}

        <StatusBar runtime={runtime} />

        {mobile && (
          <SideBar mobile collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} />
        )}

        {/* One workspace instance survives breakpoints and layout changes. */}
        <AgentWorkspace
          mode={mobile ? "drawer" : "docked"}
          dockedSlot={auxSlot}
          agents={runtime.agents}
          openTabs={runtime.openTabs}
          activeAgent={activeAgent}
          statuses={runtime.statuses}
          messages={runtime.messages[activeAgent] ?? []}
          historyLoading={runtime.historyLoading[activeAgent] ?? false}
          streaming={runtime.streaming[activeAgent] ?? null}
          usage={runtime.usage[activeAgent] ?? null}
          display={runtime.display}
          draft={runtime.draft}
          busy={runtime.busy}
          rightOpen={state.aux.inspectorOpen}
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
          onSelectAgent={runtime.selectAgent}
          onCloseAgent={runtime.closeTab}
          onSpawn={() => setSpawnOpen(true)}
          storedAgents={runtime.storedAgents}
          onSearchStoredAgents={(query) => void runtime.refreshStoredAgents(query)}
          onResumeAgent={(agent) => void runtime.resumeAgent(agent)}
          onToggleRight={toggleAuxInspector}
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

        <CommandPalette />
      </main>
    </WorkbenchRuntimeContext.Provider>
  );
}
