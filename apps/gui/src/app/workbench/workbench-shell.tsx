import { WorkbenchViewHosts } from "./view-host";
import { isWorkbenchViewVisible, useViewLayout } from "./view-layout";
/**
 * Workbench shell: title bar / activity bar / side bar / editor area /
 * bottom panel / status bar — the VSCode layout skeleton. Every window is
 * an editor tab: each visible agent chat portals into its own tab slot,
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
import { AgentWorkspace, AgentDialogs, SpawnAgentDialog, leafName } from "@/features/agent";
import { ContextBasketScope } from "@/features/context/context-basket";
import { SearchDialog } from "@/features/wiki/search-dialog";
import { closeWikiSearch, useWikiSearchOpen } from "@/features/wiki/wiki-state";
import { useMediaQuery } from "@/hooks/use-media-query";
import { cn } from "@/lib/utils";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { registerCommands } from "./command-registry";
import { CommandPalette } from "./command-palette";
import { registerWorkbenchEditorKinds } from "./editor-kinds";
import { editorKind } from "./editor-registry";
import { activeEditorContext } from "./active-editor-context";
import { parseContextMessage, serializeContext, type ContextResource } from "@/features/context/context-model";
import { useAgentSlot } from "./agent-slot-registry";
import "./static-commands";
import { useWorkbenchKeybindings } from "./keybindings";
import {
  activateEditorById,
  allGroups,
  getGroup,
  agentEditorId,
  bumpUnreadNotices,
  clearUnreadNotices,
  closeEditor,
  openAgentEditor,
  openEditor,
  openWikiEntryEditor,
  setPanelHeight,
  setSidebarCollapsed,
  setSidebarWidth,
  togglePanel,
  toggleSidebar,
  useWorkbenchState,
} from "./workbench-store";
import { openBibliographyEditor } from "./static-commands";
import { WorkbenchRuntimeContext, type AgentRuntime } from "./runtime-context";
import { ActivityBar } from "./parts/activity-bar";
import { SideBar } from "./parts/side-bar";
import { SecondarySidebar } from "./parts/secondary-sidebar";
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

/** One workspace per visible agent group, with agent-scoped input and context. */
function AgentWorkspaceHost({ path, visible, runtime, onSpawn }: { path: string; visible: boolean; runtime: AgentRuntime; onSpawn: () => void }) {
  const slot = useAgentSlot(path);
  const workbench = useWorkbenchState();
  const [targets, setTargets] = useState<Record<string, string>>({});
  const [followedEditorId, setFollowedEditorId] = useState<string | null>(null);
  const focusedEditor = activeEditorContext(workbench).editor;
  useEffect(() => {
    if (focusedEditor && editorKind(focusedEditor.kind)?.agentContext) setFollowedEditorId(focusedEditor.id);
  }, [focusedEditor]);
  const contextId = runtime.activeContexts[path] ?? runtime.contexts[path]?.id ?? null;
  useEffect(() => {
    if (visible && contextId) void runtime.ensureAgentHistory(path, contextId);
  }, [contextId, path, visible, runtime.ensureAgentHistory]);
  const refreshEffort = useCallback(() => { if (visible) void runtime.refreshAgentEffort(path); }, [runtime.refreshAgentEffort, path, visible]);
  const select = (target: string) => { runtime.selectAgent(target); openAgentEditor(target); };
  const scope = path && contextId ? JSON.stringify([path, contextId]) : null;
  const candidates = allGroups(workbench.groups).flatMap(([, group]) => group.editors).filter(editor => Boolean(editorKind(editor.kind)?.agentContext));
  useEffect(() => {
    const openIds = new Set(allGroups(workbench.groups).flatMap(([, group]) => group.editors).map(editor => editor.id));
    setTargets(current => {
      const entries = Object.entries(current).filter(([, id]) => !id || id === "$follow" || openIds.has(id));
      return entries.length === Object.keys(current).length ? current : Object.fromEntries(entries);
    });
  }, [workbench.groups]);
  const targetMode = targets[scope ?? ""] ?? "";
  const target = candidates.find(editor => editor.id === (targetMode === "$follow" ? followedEditorId : targetMode));
  const targetKind = target ? editorKind(target.kind) : undefined;
  const snapshot = target && targetKind?.agentContext?.(target);
  const resource: ContextResource | null = target && snapshot ? { kind: "editor", id: target.id, title: targetKind?.title(target) ?? target.id, scope: targetMode === "$follow" ? "最近活动文档的当前状态" : "固定编辑器的当前状态", source: target.id, revision: String(workbench.revision), content: JSON.stringify(snapshot, null, 2) } : null;
  return <ContextBasketScope scope={scope} label={leafName(path)}>
    <AgentWorkspace contextTarget={<div className="flex flex-col gap-1 border-b px-3 py-2 text-xs"><label>上下文目标 <select aria-label="Agent 上下文目标" className="ml-2 max-w-full rounded border bg-background p-1" value={targetMode} onChange={event => { if (scope) setTargets(current => ({ ...current, [scope]: event.target.value })); }}><option value="">未绑定</option><option value="$follow">跟随当前文档</option>{candidates.map(editor => <option key={editor.id} value={editor.id}>{editorKind(editor.kind)?.title(editor)}</option>)}</select></label>{resource && <details><summary>查看发送时附加的状态</summary><pre className="max-h-32 overflow-auto whitespace-pre-wrap">{resource.content}</pre></details>}</div>} dockedSlot={slot} agents={runtime.agents.some(agent => agent.path === path) ? runtime.agents : [...runtime.agents, { path, agent_id: null, status: "Idle", ...runtime.agentModels[path] }]} activeAgent={path} statuses={runtime.statuses}
      messages={runtime.messages[path] ?? []} historyLoading={runtime.historyLoading[path] ?? false}
      compaction={runtime.compactions[path]}
      streaming={runtime.streaming[path] ?? null} usage={runtime.usage[path] ?? null} display={runtime.display}
      draft={runtime.drafts[path] ?? ""} busy={runtime.sendingPaths[path] ? "send" : runtime.busy === "send" ? null : runtime.busy}
      onDraftChange={value => runtime.setDraftFor(path, value)} onSendMessage={context => {
        const attached = parseContextMessage(context ?? "")?.resources ?? [];
        void runtime.sendMessageTo(path, resource ? serializeContext([...attached, resource]) : context);
      }}
      onCancel={() => void gatewayRequest("POST", `/agents/${path}/cancel`).catch(cause => window.alert(String(cause)))}
      onCompact={() => void gatewayRequest("POST", `/agents/${path}/compact`).catch(cause => window.alert(String(cause)))}
      modelCatalog={runtime.modelCatalog} onLoadModelCatalog={runtime.loadModelCatalog} onChangeModel={runtime.setAgentModel}
      agentEffort={runtime.agentEfforts[path]} onRefreshEffort={refreshEffort} onChangeEffort={runtime.setAgentEffort}
      onSelectAgent={select} openTabs={runtime.openTabs} onCloseAgent={runtime.closeTab} onSpawn={onSpawn}
      storedAgents={runtime.storedAgents} onSearchStoredAgents={query => void runtime.refreshStoredAgents(query)}
      onResumeAgent={agent => { void runtime.resumeAgent(agent).then(() => openAgentEditor(runtime.agents.find(item => item.agent_id === agent.id)?.path ?? agent.name)).catch(cause => window.alert(String(cause))); }}
      onOpenDag={() => openEditor({ kind: "dag", params: { agentPath: path } })}
      onOpenFlow={() => openEditor({ kind: "flow", params: { agentPath: path } })} />
  </ContextBasketScope>;
}

export function WorkbenchShell({ runtime }: { runtime: AgentRuntime }) {
  const state = useWorkbenchState();
  const viewLayout = useViewLayout();
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
      if (!isWorkbenchViewVisible("notices", state, viewLayout, mobile)) bumpUnreadNotices(hiddenCount);
    }
    seenNoticesRef.current = Math.max(count, seenNoticesRef.current);
    if (isWorkbenchViewVisible("notices", state, viewLayout, mobile)) clearUnreadNotices();
  }, [runtime.notices.length, state, viewLayout, mobile]);

  // Agent tab sync (runtime openTabs/activeAgent ⇔ workbench agent tabs).
  useEffect(() => {
    const openTabs = runtime.openTabs;
    const activeAgent = runtime.activeAgent;
    const livePaths = new Set<string>();
    const activePaths = new Set<string>();
    for (const [groupId, group] of allGroups(state.groups)) {
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
      const activeGroup = getGroup(state.groups, state.groups.active);
      const activeEditor = activeGroup?.editors.find((editor) => editor.id === activeGroup?.activeId);
      if (!activeEditor || activeEditor.kind === "agent") {
        activateEditorById(agentEditorId(activeAgent));
      }
    }
  }, [runtime.openTabs, runtime.activeAgent, state.groups]);

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
  const agentEditors = allGroups(state.groups).flatMap(([, group]) => group.editors
    .filter(editor => editor.kind === "agent")
    .map(editor => ({ path: String(editor.params.path ?? ""), visible: editor.id === group.activeId }))
  ).filter(item => item.path);

  return (
    <WorkbenchRuntimeContext.Provider value={runtime}>
      <main
        data-workbench=""
        data-sidebar-collapsed={state.sidebar.collapsed || undefined}
        className="relative grid h-full min-h-0 grid-rows-[minmax(0,1fr)_auto] bg-background text-foreground"
      >
        <div className="flex min-h-0 min-w-0">
          <ActivityBar
            mobile={mobile}
            onToggleSidebar={toggleSidebar}
            onTogglePanel={togglePanel}
            onLogout={logout}
          />
          {!mobile && !state.sidebar.collapsed && (
            <div className="relative min-h-0 shrink-0" style={{ width: state.sidebar.width, maxWidth: "calc(100% - 300px)" }}>
              <SideBar mobile={false} collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} onSpawn={() => setSpawnOpen(true)} />
              <div
                role="separator"
                aria-label="调整侧栏宽度"
                aria-orientation="vertical"
                aria-valuemin={160}
                aria-valuemax={600}
                aria-valuenow={state.sidebar.width}
                tabIndex={0}
                className="absolute inset-y-0 -right-1 z-10 w-2 cursor-col-resize touch-none focus-visible:outline-none after:absolute after:inset-y-0 after:left-1/2 after:w-px after:bg-transparent hover:after:bg-primary focus-visible:after:bg-primary"
                onPointerDown={(event) => {
                  if (event.button !== 0) return;
                  event.preventDefault();
                  event.currentTarget.setPointerCapture(event.pointerId);
                }}
                onPointerMove={(event) => {
                  if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
                  const sidebar = event.currentTarget.parentElement;
                  const parent = sidebar?.parentElement;
                  if (!sidebar || !parent) return;
                  const max = Math.max(160, parent.getBoundingClientRect().right - sidebar.getBoundingClientRect().left - 300);
                  setSidebarWidth(Math.min(max, event.clientX - sidebar.getBoundingClientRect().left));
                }}
                onKeyDown={(event) => {
                  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                  event.preventDefault();
                  const sidebar = event.currentTarget.parentElement;
                  const max = sidebar?.parentElement ? Math.max(160, sidebar.parentElement.getBoundingClientRect().right - sidebar.getBoundingClientRect().left - 300) : 600;
                  setSidebarWidth(Math.min(max, state.sidebar.width + (event.key === "ArrowRight" ? 10 : -10)));
                }}
              />
            </div>
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
              <BottomPanel />
            </ResizablePanel>
          </ResizablePanelGroup>
          {!mobile && <SecondarySidebar />}
        </div>

        <WorkbenchViewHosts mobile={mobile} onSpawn={() => setSpawnOpen(true)} />
        <StatusBar runtime={runtime} />

        {mobile && (
          <SideBar mobile collapsed={state.sidebar.collapsed} onToggleCollapsed={toggleSidebar} onLogout={logout} onSpawn={() => setSpawnOpen(true)} />
        )}

        {agentEditors.map(({ path, visible }) => <AgentWorkspaceHost key={path} path={path} visible={visible} runtime={runtime} onSpawn={() => setSpawnOpen(true)} />)}

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
