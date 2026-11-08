/**
 * Side bar: the container-scoped view switcher (desktop) / overlay drawer
 * (mobile, with the edge-swipe gesture moved verbatim from the old shell).
 * M1 keeps the bibliography container as the four workspace shortcuts that
 * drive the composite bibliography editor; M2/M3 replace those with real
 * tree/list views.
 */

import { useEffect, useRef } from "react";
import { setProjectSidebarSlot } from "@/features/projects/sidebar-slot";
import { setBibliographySidebarSlot } from "@/features/bibliography/sidebar-slot";
import { Camera, Compass, Library, LogOut, Monitor, Moon, PanelBottom, PanelLeft, Rss, Search, Sun } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { AgentSidebar } from "./agent-sidebar";
import { WikiSidebarView } from "@/features/wiki/wiki-sidebar-view";
import { VfsSidebar } from "@/features/vfs";
import { DeckSidebarView } from "@/features/deck/deck-sidebar-view";
import { cn } from "@/lib/utils";
import { useTheme } from "@/hooks/use-theme";
import { cycleThemeMode, type ThemeMode } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { openEditor, setContainer, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openBibliographyEditor } from "../static-commands";
import { openPalette } from "../command-registry";
import { togglePanel } from "../workbench-store";
import { isTauri } from "@/lib/is-tauri";
import { WindowControls } from "./window-controls";



const BIB_VIEWS: readonly { id: BibliographyWorkspace; label: string; name: string; icon: LucideIcon }[] = [
  { id: "library", label: "文献库", name: "Bibliography library", icon: Library },
  { id: "discover", label: "发现", name: "Discover", icon: Compass },
  { id: "snapshots", label: "快照", name: "Snapshots", icon: Camera },
  { id: "subscriptions", label: "订阅", name: "Subscriptions", icon: Rss },
];

const CONTAINER_TITLES: Record<ContainerId, string> = {
  bibliography: "文献与证据",
  wiki: "知识库",
  agents: "Agent",
  projects: "插件项目",
  vfs: "文件",
  decks: "幻灯片",
};

function ContainerBody({ container, mobile, onSpawn }: { container: ContainerId; mobile: boolean; onSpawn: () => void }) {
  const state = useWorkbenchState();
  if (container === "bibliography") {
    const active = state.containerViews.bibliography ?? "library";
    if (!mobile) return <div ref={setBibliographySidebarSlot} className="flex min-h-0 flex-1 flex-col" />;
    return (
      <div role="group" aria-label="文献工作区" className="flex flex-col gap-0.5 p-2">
        {BIB_VIEWS.filter((view) => mobile || view.id === "library").map((view) => (
          <Button
            key={view.id}
            size="sm"
            variant={active === view.id ? "secondary" : "ghost"}
            className="w-full shrink-0 justify-start gap-2 px-2"
            aria-label={view.name}
            aria-current={active === view.id ? "page" : undefined}
            title={view.label}
            onClick={() => openBibliographyEditor(view.id)}
          >
            <view.icon />
            <span className="truncate text-xs">{view.label}</span>
          </Button>
        ))}
      </div>
    );
  }
  if (container === "projects") {
    const group = state.groups.active === "main" ? state.groups.main : state.groups.active === "secondary" ? state.groups.secondary : state.groups.extra[state.groups.active];
    const active = group?.editors.find(editor => editor.id === group.activeId);
    return <><div ref={setProjectSidebarSlot} className="flex min-h-0 flex-1 flex-col" />{active?.kind !== "plugin-projects" && <Button variant="ghost" onClick={() => openEditor({ kind: "plugin-projects" })}>打开插件项目</Button>}</>;
  }
  if (container === "wiki") {
    return (
      <div role="group" aria-label="知识库工作区" className="flex min-h-0 flex-1 flex-col">
        <WikiSidebarView onOpenGraph={() => openEditor({ kind: "wiki-graph" })} />
      </div>
    );
  }
  if (container === "vfs") {
    return <VfsSidebar />;
  }
  if (container === "decks") {
    return (
      <div role="group" aria-label="幻灯片工作区" className="flex min-h-0 flex-1 flex-col">
        <DeckSidebarView />
      </div>
    );
  }
  return <AgentSidebar onSpawn={onSpawn} />;
}

export function SideBar(props: {
  mobile: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onLogout: () => void;
  onSpawn: () => void;
}) {
  const state = useWorkbenchState();
  const container = state.sidebar.container ?? "bibliography";
  const drawerOpen = props.mobile && !props.collapsed;

  // Edge-swipe drawer (moved verbatim from app-view.tsx): swiping right
  // from the left third of the screen opens the drawer; the drawer and its
  // backdrop follow the finger, vertical scroll wins when it dominates.
  const drawerRef = useRef<HTMLElement | null>(null);
  const drawerBackdropRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (!props.mobile || !props.collapsed) return;
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
        clearDragStyles();
        props.onToggleCollapsed();
      } else if (Math.abs(dy) > 24 && Math.abs(dy) > dx) {
        tracking = false;
        clearDragStyles();
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
        clearDragStyles();
      }
    };
    const onTouchEnd = () => {
      if (tracking) clearDragStyles();
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
  }, [props.mobile, props.collapsed, props]);

  if (props.mobile) {
    return (
      <>
        <button
          type="button"
          ref={drawerBackdropRef}
          aria-label="收起侧栏"
          onClick={props.onToggleCollapsed}
          className={cn(
            "absolute inset-0 z-[35] bg-[var(--scrim)] outline-none",
            "transition-[opacity,visibility] duration-200 ease-out motion-reduce:transition-none",
            drawerOpen ? "visible opacity-100" : "invisible opacity-0 pointer-events-none",
          )}
        />
        <aside
          ref={drawerRef}
          aria-label="工作台侧栏"
          data-slot="workbench-sidebar"
          className={cn(
            "absolute inset-y-0 left-0 z-40 flex w-[min(72vw,208px)] min-h-0 flex-col border-r bg-sidebar text-sidebar-foreground shadow-2xl",
            "transition-[translate,visibility] duration-200 ease-out motion-reduce:transition-none",
            drawerOpen ? "visible translate-x-0" : "invisible -translate-x-full pointer-events-none",
          )}
        >
          <div data-tauri-drag-region="" className="flex min-h-12 w-full shrink-0 items-center gap-2 border-b px-2">
            <Button size="icon-sm" variant="ghost" className="shrink-0" aria-label="折叠侧栏" title="折叠侧栏" onClick={props.onToggleCollapsed}><PanelLeft /></Button>
            <div className="grid min-w-0 flex-1 leading-tight">
              <span className="truncate text-sm font-semibold">Autonomics</span>
              <span className="truncate text-xs text-muted-foreground">Research workspace</span>
            </div>
          </div>
          <SideBarContent container={container} mobile={props.mobile} onSpawn={props.onSpawn} />
          <Button variant="ghost" className="justify-start" onClick={() => { setContainer("projects"); openEditor({ kind: "plugin-projects" }); }}>插件项目</Button>
          <MobileDrawerFooter onLogout={props.onLogout} />
        </aside>
      </>
    );
  }

  return (
    <aside
      aria-label="工作台侧栏"
      data-slot="workbench-sidebar"
      className="flex h-full min-w-0 w-full flex-col border-r bg-sidebar text-sidebar-foreground"
    >
      <SideBarContent container={container} mobile={props.mobile} onSpawn={props.onSpawn} />
    </aside>
  );
}

function SideBarContent({ container, mobile, onSpawn }: { container: ContainerId; mobile: boolean; onSpawn: () => void }) {
  const state = useWorkbenchState();
  return (
    <>
      {container !== "wiki" && container !== "agents" && container !== "projects" && container !== "vfs" && container !== "decks" && <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <span className="truncate text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          {CONTAINER_TITLES[container]}
        </span>
        <span className="ml-auto text-[10px] text-muted-foreground/70" aria-hidden>
          {state.containerViews[container]}
        </span>
      </div>}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <ContainerBody container={container} mobile={mobile} onSpawn={onSpawn} />
      </div>
    </>
  );
}

const THEME_META: Record<ThemeMode, { label: string; icon: LucideIcon }> = {
  light: { label: "浅色主题", icon: Sun },
  dark: { label: "深色主题", icon: Moon },
  system: { label: "跟随系统", icon: Monitor },
};

/** Mobile drawer footer: theme cycle + logout (desktop keeps them in the
 *  title bar). */
function MobileDrawerFooter({ onLogout }: { onLogout: () => void }) {
  const { mode } = useTheme();
  const meta = THEME_META[mode];
  return (
    <div className="flex w-full shrink-0 flex-col gap-1 border-t p-2">
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label="搜索或输入命令" onClick={() => openPalette("all")}><Search /><span>搜索或输入命令</span></Button>
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label="切换底部面板" onClick={togglePanel}><PanelBottom /><span>切换底部面板</span></Button>
      {isTauri && <WindowControls />}
      <Button
        size="sm"
        variant="ghost"
        className="w-full justify-start gap-2"
        aria-label={`色彩主题:${meta.label},点击切换`}
        title={`色彩主题:${meta.label}(点击切换)`}
        onClick={cycleThemeMode}
      >
        <meta.icon />
        <span className="truncate">{meta.label}</span>
      </Button>
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label="退出登录" onClick={onLogout}>
        <LogOut />
        <span>退出登录</span>
      </Button>
    </div>
  );
}
