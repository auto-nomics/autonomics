import { WorkbenchViewSlot } from "../view-host";
import { useActiveEditorContext } from "../active-editor-context";
import type { LucideIcon } from "lucide-react";
/**
 * Side bar: the container-scoped view switcher (desktop) / overlay drawer
 * (mobile, with the edge-swipe gesture moved verbatim from the old shell).
 * M1 keeps the bibliography container as the four workspace shortcuts that
 * drive the composite bibliography editor; M2/M3 replace those with real
 * tree/list views.
 */

import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { LogOut, Monitor, Moon, PanelBottom, PanelLeft, Search, Sun } from "lucide-react";
import { cn } from "@/lib/utils";
import { useTheme } from "@/hooks/use-theme";
import { cycleThemeMode, type ThemeMode } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { openEditor, setContainer, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openPalette } from "../command-registry";
import { togglePanel } from "../workbench-store";
import { isTauri } from "@/lib/is-tauri";
import { WindowControls } from "./window-controls";
import "../builtin-views";
import { workbenchView, useWorkbenchViews } from "../view-registry";
import { moveWorkbenchView, useViewLayout, workbenchViewLocation } from "../view-layout";
import { startViewDrag, viewDropTarget } from "../view-drag";
import { ViewLocationMenu } from "../view-location-menu";



function ContainerBody({ container }: { container: ContainerId; mobile: boolean; onSpawn: () => void }) {
  const { t } = useTranslation("workbench");
  useWorkbenchViews();
  const layout = useViewLayout();
  const Body = workbenchView(container)?.body;
  if (!Body) return null;
  if (workbenchViewLocation(container, layout) !== "primarySidebar") return <Button variant="ghost" onClick={() => moveWorkbenchView(container, "primarySidebar")}>{t("sidebar.moveBack")}</Button>;
  return <WorkbenchViewSlot id={container} />;
}

export function SideBar(props: {
  mobile: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onLogout: () => void;
  onSpawn: () => void;
}) {
  const { t } = useTranslation("workbench");
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
          aria-label={t("sidebar.collapseAria")}
          onClick={props.onToggleCollapsed}
          className={cn(
            "absolute inset-0 z-[35] bg-[var(--scrim)] outline-none",
            "transition-[opacity,visibility] duration-200 ease-out motion-reduce:transition-none",
            drawerOpen ? "visible opacity-100" : "invisible opacity-0 pointer-events-none",
          )}
        />
        <aside {...viewDropTarget("primarySidebar")}
          ref={drawerRef}
          aria-label={t("sidebar.sidebarAria")}
          data-slot="workbench-sidebar"
          className={cn(
            "absolute inset-y-0 left-0 z-40 flex w-[min(72vw,208px)] min-h-0 flex-col border-r bg-sidebar text-sidebar-foreground shadow-2xl",
            "transition-[translate,visibility] duration-200 ease-out motion-reduce:transition-none",
            drawerOpen ? "visible translate-x-0" : "invisible -translate-x-full pointer-events-none",
          )}
        >
          <div data-tauri-drag-region="" className="flex min-h-12 w-full shrink-0 items-center gap-2 border-b px-2">
            <Button size="icon-sm" variant="ghost" className="shrink-0" aria-label={t("sidebar.collapse")} title={t("sidebar.collapse")} onClick={props.onToggleCollapsed}><PanelLeft /></Button>
            <div className="grid min-w-0 flex-1 leading-tight">
              <span className="truncate text-sm font-semibold">Autonomics</span>
              <span className="truncate text-xs text-muted-foreground">{t("sidebar.tagline")}</span>
            </div>
          </div>
          <SideBarContent container={container} mobile={props.mobile} onSpawn={props.onSpawn} />
          <Button variant="ghost" className="justify-start" onClick={() => { setContainer("projects"); openEditor({ kind: "plugin-projects" }); }}>{t("sidebar.plugins")}</Button>
          <MobileDrawerFooter onLogout={props.onLogout} />
        </aside>
      </>
    );
  }

  return (
    <aside {...viewDropTarget("primarySidebar")}
      aria-label={t("sidebar.sidebarAria")}
      data-slot="workbench-sidebar"
      className="flex h-full min-w-0 w-full flex-col border-r bg-sidebar text-sidebar-foreground"
    >
      <SideBarContent container={container} mobile={props.mobile} onSpawn={props.onSpawn} />
    </aside>
  );
}

function SideBarContent({ container, mobile, onSpawn }: { container: ContainerId; mobile: boolean; onSpawn: () => void }) {
  const { editor } = useActiveEditorContext();
  const hideBibliographyHeader = container === "bibliography" && (
    editor?.kind === "reader" ||
    (editor?.kind === "bibliography" && editor.params.workspace === "subscriptions")
  );
  return (
    <>
      {!hideBibliographyHeader && container !== "wiki" && container !== "agents" && container !== "projects" && container !== "vfs" && container !== "decks" && <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <span draggable={container === "canvases"} onDragStart={event => { if (container === "canvases") startViewDrag(event, container); }} className="truncate text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          {workbenchView(container)?.title}
        </span>
        <span className="flex-1" />
        <ViewLocationMenu id={container} />
      </div>}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <ContainerBody container={container} mobile={mobile} onSpawn={onSpawn} />
      </div>
    </>
  );
}

const THEME_META: Record<ThemeMode, { key: string; icon: LucideIcon }> = {
  light: { key: "theme.light", icon: Sun },
  dark: { key: "theme.dark", icon: Moon },
  system: { key: "theme.system", icon: Monitor },
};

/** Mobile drawer footer: theme cycle + logout (desktop keeps them in the
 *  title bar). */
function MobileDrawerFooter({ onLogout }: { onLogout: () => void }) {
  const { t } = useTranslation("workbench");
  const { mode } = useTheme();
  const meta = THEME_META[mode];
  return (
    <div className="flex w-full shrink-0 flex-col gap-1 border-t p-2">
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label={t("activityBar.search")} onClick={() => openPalette("all")}><Search /><span>{t("activityBar.search")}</span></Button>
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label={t("activityBar.togglePanel")} onClick={togglePanel}><PanelBottom /><span>{t("activityBar.togglePanel")}</span></Button>
      {isTauri && <WindowControls />}
      <Button
        size="sm"
        variant="ghost"
        className="w-full justify-start gap-2"
        aria-label={t("activityBar.themeLabel", { mode: t(meta.key) })}
        title={t("activityBar.themeTitle", { mode: t(meta.key) })}
        onClick={cycleThemeMode}
      >
        <meta.icon />
        <span className="truncate">{t(meta.key)}</span>
      </Button>
      <Button size="sm" variant="ghost" className="w-full justify-start gap-2" aria-label={t("activityBar.logout")} onClick={onLogout}>
        <LogOut />
        <span>{t("activityBar.logout")}</span>
      </Button>
    </div>
  );
}
