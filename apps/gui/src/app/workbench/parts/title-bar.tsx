/**
 * Title bar: app identity, the command center (palette entry point),
 * layout toggles and — only inside Tauri — window controls. The whole bar
 * is a drag region on desktop (Tauri frameless window); the buttons opt out
 * by simply not carrying the drag attribute.
 */

import { useEffect, useState } from "react";
import {
  LogOut,
  Maximize2,
  Minus,
  Monitor,
  Moon,
  PanelBottom,
  PanelLeft,
  Sun,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { isTauri } from "@/lib/is-tauri";
import { useTheme } from "@/hooks/use-theme";
import { cycleThemeMode, type ThemeMode } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { openPalette } from "../command-registry";
import { CommandCenterButton } from "../command-palette";

const THEME_META: Record<ThemeMode, { label: string; icon: LucideIcon }> = {
  light: { label: "浅色主题", icon: Sun },
  dark: { label: "深色主题", icon: Moon },
  system: { label: "跟随系统", icon: Monitor },
};

function ThemeButton() {
  const { mode } = useTheme();
  const meta = THEME_META[mode];
  return (
    <Button
      size="icon-sm"
      variant="ghost"
      aria-label={`色彩主题:${meta.label},点击切换`}
      title={`色彩主题:${meta.label}(点击切换)`}
      onClick={cycleThemeMode}
    >
      <meta.icon />
    </Button>
  );
}

function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const window = getCurrentWindow();
        setMaximized(await window.isMaximized());
        const promise = window.onResized(async () => setMaximized(await window.isMaximized()));
        unlisten = () => void promise.then((fn) => fn());
      } catch {
        /* non-Tauri build: controls are not rendered anyway */
      }
    })();
    return () => unlisten?.();
  }, []);

  const withWindow = (action: (window: import("@tauri-apps/api/window").Window) => void) => () => {
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        action(getCurrentWindow());
      } catch { /* not in Tauri */ }
    })();
  };

  return (
    <div className="flex items-center">
      <Button size="icon-sm" variant="ghost" aria-label="最小化" title="最小化" onClick={withWindow((w) => w.minimize())}><Minus /></Button>
      <Button size="icon-sm" variant="ghost" aria-label={maximized ? "还原" : "最大化"} title={maximized ? "还原" : "最大化"} onClick={withWindow((w) => w.toggleMaximize())}>
        {maximized ? <MinimizeIcon /> : <Maximize2 />}
      </Button>
      <Button size="icon-sm" variant="ghost" aria-label="关闭窗口" title="关闭" className="hover:bg-destructive hover:text-white" onClick={withWindow((w) => w.close())}><X /></Button>
    </div>
  );
}

function MinimizeIcon() {
  // Restore (unmaximize): two overlapping rectangles.
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="size-4" aria-hidden>
      <path d="M8 8h12v12H8z" />
      <path d="M4 16V4h12" />
    </svg>
  );
}

export function TitleBar(props: {
  mobile: boolean;
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  panelVisible: boolean;
  onTogglePanel: () => void;
  onLogout: () => void;
}) {
  return (
    <header
      data-tauri-drag-region=""
      className="flex h-9 shrink-0 items-center gap-1.5 border-b bg-sidebar px-1.5 text-sidebar-foreground select-none max-sm:h-10"
    >
      <div className="grid size-6 shrink-0 place-items-center rounded-md bg-sidebar-primary text-xs font-bold text-sidebar-primary-foreground">A</div>
      <span className="shrink-0 truncate text-sm font-semibold max-sm:hidden">Autonomics</span>
      <div data-tauri-drag-region="" className="flex min-w-0 flex-1 justify-center px-2">
        {!props.mobile && <CommandCenterButton onClick={() => openPalette("all")} />}
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {!props.mobile && (
          <>
            <Button size="icon-sm" variant="ghost" aria-label={props.sidebarCollapsed ? "展开侧栏" : "折叠侧栏"} title={props.sidebarCollapsed ? "展开侧栏(Ctrl+B)" : "折叠侧栏(Ctrl+B)"} onClick={props.onToggleSidebar}>
              <PanelLeft />
            </Button>
            <Button size="icon-sm" variant="ghost" aria-label={props.panelVisible ? "收起面板" : "打开面板"} aria-pressed={props.panelVisible} title={props.panelVisible ? "收起面板(Ctrl+J)" : "打开面板(Ctrl+J)"} onClick={props.onTogglePanel}>
              <PanelBottom />
            </Button>
            <span className="mx-1 h-4 w-px bg-border" aria-hidden />
            <ThemeButton />
            <Button size="icon-sm" variant="ghost" aria-label="退出登录" title="退出登录" onClick={props.onLogout}><LogOut /></Button>
          </>
        )}
        {isTauri && <WindowControls />}
      </div>
    </header>
  );
}
