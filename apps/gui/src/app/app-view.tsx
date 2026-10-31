// App shell: activity navigation, persistent editors, auxiliary hosting and the
// login/boot screens. All agent state and gateway wiring lives in
// use-agent-runtime; this component only renders it.
import { useCallback, useEffect, useRef, useState } from "react";
import { Bot, Library, Network, Settings, PanelRight, PanelLeft, LogOut } from "lucide-react";
import { cn } from "@/lib/utils";
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from "@/components/ui/resizable";
import { ContextBasketProvider } from "@/features/context/context-basket";
import { gatewayRequest } from "@/gateway";
import { useAgentRuntime } from "@/app/use-agent-runtime";
import { AgentView, SpawnAgentDialog, AgentWorkspace, layoutSideOpen, readAgentLayout } from "@/features/agent";
import { BibliographyView } from "@/features/bibliography";
import { ConfigView, DagView, KmsView } from "@/features/system";
import { useMediaQuery } from "@/hooks/use-media-query";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog";

type View = "agent" | "config" | "bib" | "kms";

export function App() {
  const runtime = useAgentRuntime();
  const [view, setCurrentView] = useState<View>("agent");
  const [visited, setVisited] = useState<Set<View>>(() => new Set(["agent"]));
  const setView = (next: View) => {
    setVisited(current => current.has(next) ? current : new Set([...current, next]));
    setCurrentView(next);
  };
  const [auxiliarySlot, setAuxiliarySlot] = useState<HTMLDivElement | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try {
      if (localStorage.getItem("autonomics.workbench.sidebar.collapsed") === "true") return true;
      // Mobile has no persistent rail; it starts with the drawer closed.
      return window.innerWidth < 768;
    } catch { return false; }
  });
  useEffect(() => {
    try { localStorage.setItem("autonomics.workbench.sidebar.collapsed", String(sidebarCollapsed)); } catch { /* storage unavailable */ }
  }, [sidebarCollapsed]);
  // The single breakpoint: ≥ md is desktop (rail sidebar, auxiliary dock,
  // resizable inspector), below it everything goes mobile (drawer sidebar,
  // fullscreen workspace drawer).
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const [placement, setPlacement] = useState<"docked" | "hidden">(() => {
    try {
      // The retired "floating" placement collapses into "docked": with the
      // draggable shell gone, the drawer overlay is the non-docked hosting.
      const saved = localStorage.getItem("autonomics.workbench.agent.placement");
      return saved === "hidden" ? "hidden" : "docked";
    } catch { return "docked"; }
  });
  const [auxiliarySize] = useState(() => {
    try {
      const value = Number(localStorage.getItem("autonomics.workbench.agent.width"));
      return value >= 20 && value <= 60 ? value : 34;
    } catch { return 34; }
  });
  const auxiliarySizeRef = useRef(auxiliarySize);
  useEffect(() => {
    try { localStorage.setItem("autonomics.workbench.agent.placement", placement); } catch { /* storage unavailable */ }
  }, [placement]);
  const showAuxiliary = view !== "agent" && placement === "docked" && isDesktop;
  const [spawnOpen, setSpawnOpen] = useState(false);
  const [dagAgent, setDagAgent] = useState<string | null>(null);
  // Workspace hosting: the center column is a single AgentWorkspace
  // instance that portals into the main editor, auxiliary pane or bottom
  // drawer according to the selected view and placement.
  const [dockedSlot, setDockedSlot] = useState<HTMLDivElement | null>(null);
  const workspaceMode = view === "agent" ? (dockedSlot ? "docked" : "standby")
    : placement === "hidden" ? "standby"
    : showAuxiliary ? (auxiliarySlot ? "docked" : "standby") : "drawer";
  // Mobile renders no persistent rail: the collapsed state hides the sidebar
  // entirely and a blank left-edge strip re-opens it as an overlay drawer.
  // Desktop keeps the 56px icon rail with its toggle button.
  const mobile = !isDesktop;
  const railCollapsed = sidebarCollapsed && isDesktop;
  const drawerOpen = mobile && !sidebarCollapsed;
  const closeDrawer = () => setSidebarCollapsed(true);
  const drawerRef = useRef<HTMLElement | null>(null);
  const drawerBackdropRef = useRef<HTMLButtonElement | null>(null);
  // With the drawer closed on mobile, swiping right from anywhere in the left
  // third of the screen opens it. The drawer and its backdrop follow the
  // finger while the gesture lasts; opening still commits at the same
  // threshold, vertical scrolling wins as soon as it dominates, and anything
  // else snaps back, so ordinary content scrolls never trigger the drawer.
  useEffect(() => {
    if (!mobile || !sidebarCollapsed) return;
    let startX = 0, startY = 0, tracking = false, drawerWidth = 0;
    const clearDragStyles = () => {
      const drawer = drawerRef.current;
      const backdrop = drawerBackdropRef.current;
      if (drawer) { drawer.style.transition = ""; drawer.style.translate = ""; drawer.style.visibility = ""; }
      if (backdrop) { backdrop.style.transition = ""; backdrop.style.opacity = ""; backdrop.style.visibility = ""; }
    };
    const onTouchStart = (event: TouchEvent) => {
      if (!document.querySelector("main[data-workbench]")) return; // login/boot screens
      const touch = event.touches[0];
      tracking = touch.clientX <= window.innerWidth / 3;
      startX = touch.clientX;
      startY = touch.clientY;
      drawerWidth = drawerRef.current?.offsetWidth ?? 0;
    };
    const onTouchMove = (event: TouchEvent) => {
      if (!tracking) return;
      const touch = event.touches[0];
      const dx = touch.clientX - startX;
      const dy = touch.clientY - startY;
      if (dx >= 48 && dx > Math.abs(dy) * 1.5) {
        tracking = false;
        clearDragStyles(); // settled classes take over from the dragged position
        setSidebarCollapsed(false);
      } else if (Math.abs(dy) > 24 && Math.abs(dy) > dx) {
        tracking = false; // the gesture turned into a vertical scroll
        clearDragStyles(); // snap the preview back
      } else if (dx > 0) {
        const drawer = drawerRef.current;
        const backdrop = drawerBackdropRef.current;
        const reveal = Math.min(dx, drawerWidth);
        if (drawer) {
          drawer.style.transition = "none";
          drawer.style.visibility = "visible";
          drawer.style.translate = `${reveal - drawerWidth}px 0`;
        }
        if (backdrop) {
          backdrop.style.transition = "none";
          backdrop.style.visibility = "visible";
          backdrop.style.opacity = drawerWidth ? String(reveal / drawerWidth) : "0";
        }
      } else {
        clearDragStyles(); // finger back at or behind the start point
      }
    };
    const onTouchEnd = () => {
      if (tracking) clearDragStyles(); // released before the threshold: snap back
      tracking = false;
    };
    document.addEventListener("touchstart", onTouchStart, { passive: true });
    document.addEventListener("touchmove", onTouchMove, { passive: true });
    document.addEventListener("touchend", onTouchEnd, { passive: true });
    document.addEventListener("touchcancel", onTouchEnd, { passive: true });
    return () => {
      document.removeEventListener("touchstart", onTouchStart);
      document.removeEventListener("touchmove", onTouchMove);
      document.removeEventListener("touchend", onTouchEnd);
      document.removeEventListener("touchcancel", onTouchEnd);
    };
  }, [mobile, sidebarCollapsed]);
  const savedLayout = useRef(readAgentLayout()).current;
  const [rightOpen, setRightOpen] = useState(() => isDesktop && layoutSideOpen(savedLayout, "inspector"));

  // Viewport crossings reset the sidebar: drawer closed on mobile, panel
  // open on desktop. The first run is skipped so the restored layout — not
  // the default — stays in charge on initial load.
  const crossedOnceRef = useRef(false);
  useEffect(() => {
    if (!crossedOnceRef.current) {
      crossedOnceRef.current = true;
      return;
    }
    setRightOpen(isDesktop);
    // Entering mobile closes the drawer; entering desktop keeps the rail as-is.
    if (!isDesktop) setSidebarCollapsed(true);
  }, [isDesktop]);

  // Stable callback ref for the workspace slot; the guard avoids the
  // detach/attach churn a fresh closure would cause on every render.
  const onSlotChange = useCallback((slot: HTMLDivElement | null) => {
    setDockedSlot((current) => (current === slot ? current : slot));
  }, []);

  // Effort refresh keyed to the active agent; a fresh closure per render
  // would re-fire the workspace's effect (and refetch) on every render.
  const refreshEffort = useCallback(() => {
    void runtime.refreshAgentEffort(runtime.activeAgent);
  }, [runtime.refreshAgentEffort, runtime.activeAgent]);

  if (runtime.booting) {
    return <div className="grid h-full place-items-center text-sm text-muted-foreground">正在连接 Gateway…</div>;
  }

  if (runtime.needsLogin) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">需要 Gateway 令牌</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            令牌在运行 Gateway 的机器上:~/.autonomics/gateway.token
            (或环境变量 AUTONOMICS_HTTP_API_TOKEN)。令牌只提交一次,
            之后由 HttpOnly 会话 Cookie 维持登录。
          </p>
          <form
            className="mt-4 flex items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void runtime.submitLogin();
            }}
          >
            <Input
              type="password"
              autoComplete="off"
              autoFocus
              placeholder="Gateway 令牌"
              value={runtime.loginToken}
              onChange={(event) => runtime.setLoginToken(event.target.value)}
            />
            <Button type="submit" disabled={runtime.loginBusy || !runtime.loginToken.trim()}>
              {runtime.loginBusy ? "连接中…" : "连接"}
            </Button>
          </form>
          {runtime.loginError ? (
            <pre className="mt-3 text-sm text-destructive">{runtime.loginError}</pre>
          ) : null}
        </Card>
      </div>
    );
  }

  if (!runtime.connected && !runtime.agents.length) {
    return (
      <div className="grid h-full place-items-center p-8">
        <Card className="w-full max-w-lg p-6">
          <h1 className="text-xl font-medium">Gateway 未运行</h1>
          <pre className="mt-3 text-sm text-destructive">{runtime.connectionError}</pre>
        </Card>
      </div>
    );
  }

  return (
    <ContextBasketProvider
      scope={runtime.activeAgent && runtime.activeSessionId ? JSON.stringify([runtime.activeAgent, runtime.activeSessionId]) : null}
      sessionLabel={`${runtime.activeAgent.split("/").pop() || "未选择 Agent"} · ${(runtime.sessions[runtime.activeAgent] ?? []).find(session => session.id === runtime.activeSessionId)?.title || runtime.activeSessionId || "未选择会话"}`}
      onAdd={() => { if (placement === "hidden") setPlacement("docked"); }}
    >
    <main data-workbench="" data-sidebar-collapsed={sidebarCollapsed || undefined} className={cn("relative grid h-full min-h-0 grid-rows-[minmax(0,1fr)] bg-background", mobile ? "grid-cols-[minmax(0,1fr)]" : railCollapsed ? "grid-cols-[56px_minmax(0,1fr)]" : "grid-cols-[136px_minmax(0,1fr)] sm:grid-cols-[160px_minmax(0,1fr)] xl:grid-cols-[208px_minmax(0,1fr)]")}>
      {mobile && sidebarCollapsed && (
        <button
          type="button"
          aria-label="展开侧栏"
          title="展开侧栏"
          onClick={() => setSidebarCollapsed(false)}
          className="absolute inset-y-0 left-0 z-30 w-4 outline-none transition-colors hover:bg-accent/25"
        />
      )}
      {mobile && (
        <button
          type="button"
          ref={drawerBackdropRef}
          aria-label="收起侧栏"
          onClick={closeDrawer}
          className={cn(
            "absolute inset-0 z-[35] bg-black/50 outline-none",
            "transition-[opacity,visibility] duration-200 ease-out motion-reduce:transition-none",
            drawerOpen ? "visible opacity-100" : "invisible opacity-0 pointer-events-none",
          )}
        />
      )}
      <aside
        ref={drawerRef}
        aria-label="工作台侧栏"
        data-slot="workbench-sidebar"
        className={cn(
          "flex min-h-0 flex-col border-r bg-sidebar text-sidebar-foreground",
          mobile
            ? cn(
                "absolute inset-y-0 left-0 z-40 w-[min(72vw,208px)] shadow-2xl",
                "transition-[translate,visibility] duration-200 ease-out motion-reduce:transition-none",
                drawerOpen ? "visible translate-x-0" : "invisible -translate-x-full pointer-events-none",
              )
            : cn("min-w-0", railCollapsed && "items-center"),
        )}
      >
        <div className={cn("flex min-h-16 w-full shrink-0 items-center gap-2 border-b px-2", railCollapsed && "justify-center px-1")}>
          <Button size="icon-sm" variant="ghost" className="shrink-0" aria-label={railCollapsed ? "展开侧栏" : "折叠侧栏"} title={railCollapsed ? "展开侧栏" : "折叠侧栏"} onClick={() => setSidebarCollapsed(value => !value)}><PanelLeft /></Button>
          {!railCollapsed && <>
            <div className="grid size-8 shrink-0 place-items-center rounded-lg bg-sidebar-primary text-sm font-semibold text-sidebar-primary-foreground">A</div>
            <div className="grid min-w-0 flex-1 leading-tight"><span className="truncate text-sm font-semibold">Autonomics</span><span className="truncate text-xs text-muted-foreground">Research workspace</span></div>
          </>}
        </div>
        <nav aria-label="工作台活动栏" className={cn("flex min-h-0 w-full flex-1 flex-col gap-5 overflow-y-auto p-2", railCollapsed && "items-center px-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden")}>
          <section className={cn("flex w-full flex-col gap-1", railCollapsed && "items-center")}>
            {!railCollapsed && <h2 className="px-3 pb-1 pt-2 text-xs font-medium text-muted-foreground">工作区</h2>}
            {([{ id: "agent", label: "Agent", icon: Bot }, { id: "bib", label: "文献与证据", icon: Library }, { id: "kms", label: "知识管理", icon: Network }, { id: "config", label: "设置", icon: Settings }] as const).map(item => (
              <Button key={item.id} variant={view === item.id ? "secondary" : "ghost"} className={cn("h-9 w-full shrink-0 justify-start gap-2 px-2.5", railCollapsed && "h-9 w-9 min-w-9 justify-center p-0")} aria-label={item.id === "bib" ? "Bibliography" : item.id === "kms" ? "KMS" : item.id === "config" ? "CONFIG" : "Agent"} aria-current={view === item.id ? "page" : undefined} title={railCollapsed ? item.label : undefined} onClick={() => { setView(item.id); if (mobile) closeDrawer(); }}>
                <item.icon /><span className={cn("truncate text-xs sm:text-sm", railCollapsed && "sr-only")}>{item.label}</span>
              </Button>
            ))}
          </section>
        </nav>
        <div className={cn("flex w-full shrink-0 flex-col gap-2 border-t p-2", railCollapsed && "items-center px-1")}>
          <div className={cn("flex min-w-0 items-center gap-2 px-1", railCollapsed && "justify-center")} title={runtime.connected ? "Gateway 已连接" : "Gateway 已断开"}>
            <span aria-hidden="true" className={cn("size-2 shrink-0 rounded-full", runtime.connected ? "bg-primary" : "bg-destructive")} />
            {!railCollapsed && <span className="truncate text-xs text-muted-foreground">{runtime.connected ? "Gateway 已连接" : "Gateway 已断开"}</span>}
          </div>
          <Button
            size="sm"
            variant={placement === "docked" ? "secondary" : "ghost"}
            className={cn("w-full justify-start gap-2", railCollapsed && "h-9 w-9 min-w-9 justify-center p-0")}
            aria-label="Agent 副窗"
            aria-pressed={placement === "docked"}
            title={`切换为 Agent ${placement === "docked" ? "隐藏" : "停靠"}`}
            onClick={() => setPlacement(current => current === "docked" ? "hidden" : "docked")}
          >
            <PanelRight />
            <span className={cn("truncate", railCollapsed && "sr-only")}>{placement === "docked" ? "Agent 停靠" : "Agent 隐藏"}</span>
          </Button>
          <Button size="sm" variant="ghost" className={cn("w-full justify-start gap-2", railCollapsed && "h-9 w-9 min-w-9 justify-center p-0")} title={railCollapsed ? "退出登录" : undefined} aria-label="退出登录" onClick={() => void runtime.logout()}><LogOut />{!railCollapsed && <span>退出登录</span>}</Button>
        </div>
      </aside>
      <section className="flex min-h-0 min-w-0 flex-col overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
      <ResizablePanelGroup orientation="horizontal" onLayoutChanged={layout => {
        const size = layout["agent-assistant"];
        if (showAuxiliary && size >= 20 && size <= 60) {
          auxiliarySizeRef.current = size;
          try { localStorage.setItem("autonomics.workbench.agent.width", String(size)); } catch { /* storage unavailable */ }
        }
      }}>
      <ResizablePanel id="main-editor" minSize="35%" className="flex min-w-0 flex-col">
      {view === "agent" ? (
        <AgentView
          activeAgent={runtime.activeAgent}
          activeSessionId={runtime.activeSessionId}
          agents={runtime.agents}
          sessions={runtime.sessions[runtime.activeAgent] ?? []}
          statuses={runtime.statuses}
          messages={runtime.messages[runtime.activeAgent] ?? []}
          plan={runtime.plans[runtime.activeAgent] ?? null}
          usage={runtime.usage[runtime.activeAgent] ?? null}
          display={runtime.display}
          promptHistory={runtime.promptHistories[runtime.activeAgent] ?? []}
          rawMessages={runtime.rawMessages[`${runtime.activeAgent}:${runtime.activeSessionId}`] ?? []}
          rightOpen={rightOpen}
          isDesktop={isDesktop}
          onSetRight={setRightOpen}
          onSlotChange={onSlotChange}
          onSpawn={() => setSpawnOpen(true)}
          onDraftChange={runtime.setDraft}
          onCancel={() => void gatewayRequest("POST", `/agents/${runtime.activeAgent}/cancel`).catch((cause) => window.alert(String(cause)))}
          onCompact={() => void gatewayRequest("POST", `/agents/${runtime.activeAgent}/compact`).catch((cause) => window.alert(String(cause)))}
          onShutdown={(path) => void runtime.shutdownAgent(path)}
          onSpawnAgent={runtime.spawnAgent}
          onResumeAgent={runtime.resumeAgent}
          onCreateSession={runtime.createSession}
          onRenameSession={runtime.renameSession}
          onCloseSession={runtime.closeSession}
          onActivateSession={(sessionId) => runtime.activateSession(runtime.activeAgent, sessionId)}
          onForkMessage={runtime.forkMessage}
          onToggleDisplay={(key) => void runtime.toggleDisplay(key)}
          onView={setView}
          onOpenDag={() => { if (runtime.activeAgent) setDagAgent(runtime.activeAgent); }}
          onRefreshHistory={runtime.refreshHistory}
        />
      ) : null}
      {visited.has("bib") && <section aria-label="文献工作区" hidden={view !== "bib"} className={cn("min-h-0 min-w-0 flex-1 flex-col", view === "bib" ? "flex" : "hidden")}><BibliographyView /></section>}
      {visited.has("kms") && <section aria-label="知识工作区" hidden={view !== "kms"} className={cn("min-h-0 min-w-0 flex-1 flex-col p-3", view === "kms" ? "flex" : "hidden")}><KmsView /></section>}
      {view === "config" && <section className="flex min-h-0 flex-1 flex-col overflow-auto p-3"><ConfigView activeAgent={runtime.activeAgent} /></section>}
      </ResizablePanel>
      {showAuxiliary && <ResizableHandle aria-label="调整 Agent 辅助区宽度" />}
      {showAuxiliary && <ResizablePanel id="agent-assistant" defaultSize={`${auxiliarySizeRef.current}%`} minSize="320px" maxSize="60%" className="flex min-w-0 flex-col">
        <aside aria-label="Agent 辅助区" className="flex min-h-0 flex-1 flex-col">
          <div ref={setAuxiliarySlot} className="flex min-h-0 flex-1 flex-col" />
        </aside>
      </ResizablePanel>}
      </ResizablePanelGroup>
        </div>
      </section>

      <Dialog open={dagAgent !== null} onOpenChange={(open) => { if (!open) setDagAgent(null); }}>
        <DialogContent className="flex h-[min(80vh,800px)] w-[min(94vw,1100px)] max-w-none flex-col sm:max-w-none">
          <DialogHeader>
            <DialogTitle>Agent DAG</DialogTitle>
            <DialogDescription>{dagAgent}</DialogDescription>
          </DialogHeader>
          {dagAgent && <DagView key={dagAgent} activeAgent={dagAgent} />}
        </DialogContent>
      </Dialog>

      <SpawnAgentDialog
        open={spawnOpen}
        onOpenChange={setSpawnOpen}
        busy={runtime.busy}
        onSpawnAgent={runtime.spawnAgent}
      />

      {/* One workspace instance survives navigation and placement changes. */}
      <AgentWorkspace
        mode={workspaceMode}
        dockedSlot={view === "agent" ? dockedSlot : auxiliarySlot}
        showInspectorToggle={view === "agent"}
        agents={runtime.agents}
        activeAgent={runtime.activeAgent}
        statuses={runtime.statuses}
        messages={runtime.messages[runtime.activeAgent] ?? []}
        historyLoading={runtime.historyLoading[runtime.activeAgent] ?? false}
        streaming={runtime.streaming[runtime.activeAgent] ?? null}
        usage={runtime.usage[runtime.activeAgent] ?? null}
        display={runtime.display}
        draft={runtime.draft}
        busy={runtime.busy}
        rightOpen={rightOpen}
        onDraftChange={runtime.setDraft}
        onSendMessage={(context) => void runtime.sendMessage(context)}
        modelCatalog={runtime.modelCatalog}
        onLoadModelCatalog={runtime.loadModelCatalog}
        onChangeModel={(path, spec) => runtime.setAgentModel(path, spec)}
        agentEffort={runtime.agentEfforts[runtime.activeAgent]}
        onRefreshEffort={refreshEffort}
        onChangeEffort={(path, effort) => runtime.setAgentEffort(path, effort)}
        onSelectAgent={runtime.selectAgent}
        onShutdown={(path) => void runtime.shutdownAgent(path)}
        onSpawn={() => setSpawnOpen(true)}
        onToggleRight={() => setRightOpen((current) => !current)}
        onOpenDag={() => { if (runtime.activeAgent) setDagAgent(runtime.activeAgent); }}
      />
    </main>
    </ContextBasketProvider>
  );
}
