/**
 * Side bar: the container-scoped view switcher (desktop) / overlay drawer
 * (mobile, with the edge-swipe gesture moved verbatim from the old shell).
 * M1 keeps the bibliography container as the four workspace shortcuts that
 * drive the composite bibliography editor; M2/M3 replace those with real
 * tree/list views.
 */

import { useEffect, useRef } from "react";
import { BookOpen, Camera, Compass, Library, LogOut, Monitor, Moon, PanelLeft, Rss, Search, Sun } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { cn } from "@/lib/utils";
import { useTheme } from "@/hooks/use-theme";
import { cycleThemeMode, type ThemeMode } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { openEditor, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openBibliographyEditor } from "../static-commands";

type ContainerView = { id: string; label: string; name: string; icon: LucideIcon };

const BIB_VIEWS: readonly { id: BibliographyWorkspace; label: string; name: string; icon: LucideIcon }[] = [
  { id: "library", label: "文献库", name: "Bibliography library", icon: Library },
  { id: "discover", label: "发现", name: "Discover", icon: Compass },
  { id: "snapshots", label: "快照", name: "Snapshots", icon: Camera },
  { id: "subscriptions", label: "订阅", name: "Subscriptions", icon: Rss },
];

const WIKI_VIEWS: readonly ContainerView[] = [
  { id: "browse", label: "浏览知识库", name: "Wiki browse", icon: BookOpen },
  { id: "search", label: "检索(Ctrl+K)", name: "Wiki search", icon: Search },
];

const CONTAINER_TITLES: Record<ContainerId, string> = {
  bibliography: "文献与证据",
  wiki: "知识库",
};

function ContainerBody({ container }: { container: ContainerId }) {
  const state = useWorkbenchState();
  if (container === "bibliography") {
    const active = state.containerViews.bibliography ?? "library";
    return (
      <div role="group" aria-label="文献工作区" className="flex flex-col gap-0.5 p-2">
        {BIB_VIEWS.map((view) => (
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
  const active = state.containerViews.wiki ?? "browse";
  return (
    <div role="group" aria-label="知识库工作区" className="flex flex-col gap-0.5 p-2">
      {WIKI_VIEWS.map((view) => (
        <Button
          key={view.id}
          size="sm"
          variant={active === view.id ? "secondary" : "ghost"}
          className="w-full shrink-0 justify-start gap-2 px-2"
          aria-label={view.name}
          aria-current={active === view.id ? "page" : undefined}
          title={view.label}
          onClick={() => {
            // M1: browse/search live inside the composite wiki editor; the
            // sidebar shortcuts open it (Ctrl+K reaches the search dialog).
            openEditor({ kind: "wiki" });
          }}
        >
          <view.icon />
          <span className="truncate text-xs">{view.label}</span>
        </Button>
      ))}
    </div>
  );
}

export function SideBar(props: {
  mobile: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onLogout: () => void;
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
          <div className="flex min-h-12 w-full shrink-0 items-center gap-2 border-b px-2">
            <Button size="icon-sm" variant="ghost" className="shrink-0" aria-label="折叠侧栏" title="折叠侧栏" onClick={props.onToggleCollapsed}><PanelLeft /></Button>
            <div className="grid min-w-0 flex-1 leading-tight">
              <span className="truncate text-sm font-semibold">Autonomics</span>
              <span className="truncate text-xs text-muted-foreground">Research workspace</span>
            </div>
          </div>
          <SideBarContent container={container} />
          <MobileDrawerFooter onLogout={props.onLogout} />
        </aside>
      </>
    );
  }

  return (
    <aside
      aria-label="工作台侧栏"
      data-slot="workbench-sidebar"
      className="flex w-[220px] shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground"
    >
      <SideBarContent container={container} />
    </aside>
  );
}

function SideBarContent({ container }: { container: ContainerId }) {
  const state = useWorkbenchState();
  return (
    <>
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <span className="truncate text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          {CONTAINER_TITLES[container]}
        </span>
        <span className="ml-auto text-[10px] text-muted-foreground/70" aria-hidden>
          {state.containerViews[container]}
        </span>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <ContainerBody container={container} />
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
