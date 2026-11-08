/**
 * Activity bar: the 48px icon rail. Top = sidebar containers (literature,
 * knowledge base, agent overview); bottom = panel toggle and settings.
 * Clicking the active container icon hides the side bar (VSCode
 * behavior); clicking it again reveals it.
 */

import { BookOpen, Bot, Camera, Compass, FolderCode, FolderTree, Library, LogOut, Monitor, Moon, PanelBottom, PanelLeft, Rss, Search, Settings, Sun } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { setContainer, setSidebarCollapsed, openEditor, getGroup, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openBibliographyEditor } from "../static-commands";
import { openPalette } from "../command-registry";
import { useTheme } from "@/hooks/use-theme";
import { cycleThemeMode } from "@/lib/theme";
import { isTauri } from "@/lib/is-tauri";
import { WindowControls } from "./window-controls";

function RailButton(props: {
  label: string;
  title: string;
  icon: LucideIcon;
  active?: boolean;
  pressed?: boolean;
  badge?: boolean;
  onClick: () => void;
}) {
  return (
    <Button
      size="icon"
      variant="ghost"
      className={cn(
        "relative size-10 shrink-0 rounded-none text-muted-foreground",
        // VSCode-style active marker: a 2px accent bar on the left edge.
        props.active && "text-foreground shadow-[inset_2px_0_0_0] shadow-primary",
      )}
      aria-label={props.label}
      aria-current={props.active ? "page" : undefined}
      aria-pressed={props.pressed}
      title={props.title}
      onClick={props.onClick}
    >
      <props.icon className="size-5" />
      {props.badge && <span aria-hidden className="absolute right-1.5 top-1.5 size-2 rounded-full bg-primary" />}
    </Button>
  );
}

export function ActivityBar(props: {
  mobile: boolean;
  onToggleSidebar: () => void;
  onTogglePanel: () => void;
  onLogout: () => void;
}) {
  const state = useWorkbenchState();
  const { mode } = useTheme();
  const theme = mode === "light" ? { icon: Sun, label: "浅色主题" } : mode === "dark" ? { icon: Moon, label: "深色主题" } : { icon: Monitor, label: "跟随系统" };
  const collapsed = state.sidebar.collapsed;

  const containerClick = (container: ContainerId) => {
    // Rail container clicks ALWAYS reveal the side bar with that
    // container — collapsing is the title-bar button / Ctrl+B's job. This
    // keeps repeated clicks idempotent (browse lists stay reachable).
    setContainer(container);
    setSidebarCollapsed(false);
    if (container === "bibliography") openBibliographyEditor();
    if (container === "projects") openEditor({ kind: "plugin-projects" });
  };

  const group = getGroup(state.groups, state.groups.active);
  const activeEditor = group?.editors.find((editor) => editor.id === group.activeId);
  const activeWorkspace = activeEditor?.kind === "bibliography" ? (activeEditor.params.workspace ?? "library") : null;
  const bibActive = !collapsed && state.sidebar.container === "bibliography" && (activeWorkspace === "library" || activeWorkspace === null);
  const wikiActive = !collapsed && state.sidebar.container === "wiki";
  const agentsActive = !collapsed && state.sidebar.container === "agents";
  const vfsActive = !collapsed && state.sidebar.container === "vfs";
  const pendingNotices = state.panel.unreadNotices > 0 && (!state.panel.visible || state.panel.tab !== "notices");

  return (
    <nav
      aria-label="工作台活动栏"
      className={cn("flex w-12 min-h-0 shrink-0 flex-col items-center overflow-y-auto border-r bg-sidebar py-1 text-sidebar-foreground", props.mobile && "hidden")}
    >
      <div className="flex flex-col items-center gap-0.5">
        <RailButton
          label="Bibliography"
          title="文献与证据"
          icon={Library}
          active={bibActive}
          onClick={() => containerClick("bibliography")}
        />
        <RailButton label="Discover" title="发现" icon={Compass} active={activeWorkspace === "discover"} onClick={() => openBibliographyEditor("discover")} />
        <RailButton label="Snapshots" title="快照" icon={Camera} active={activeWorkspace === "snapshots"} onClick={() => openBibliographyEditor("snapshots")} />
        <RailButton label="Subscriptions" title="订阅" icon={Rss} active={activeWorkspace === "subscriptions"} onClick={() => openBibliographyEditor("subscriptions")} />
        <RailButton
          label="Wiki"
          title="知识库(条目即编辑器标签,Ctrl+K 检索)"
          icon={BookOpen}
          active={wikiActive}
          onClick={() => containerClick("wiki")}
        />
        <RailButton label="插件管理" title="插件管理" icon={FolderCode} onClick={() => openEditor({ kind: "plugin-manager" })} />
        <RailButton label="插件项目" title="插件项目" icon={FolderCode} active={!collapsed && state.sidebar.container === "projects"} onClick={() => containerClick("projects")} />
        <RailButton
          label="VFS 文件"
          title="VFS 文件浏览器（挂载 / 容器输出 / 数据）"
          icon={FolderTree}
          active={vfsActive}
          onClick={() => containerClick("vfs")}
        />
        <RailButton
          label="Agent"
          title="Agent 概览(对话即编辑器标签)"
          icon={Bot}
          active={agentsActive}
          onClick={() => containerClick("agents")}
        />
      </div>
      <div data-tauri-drag-region="" className="min-h-2 w-full flex-1" />
      <div className="flex shrink-0 flex-col items-center gap-0.5">
        <RailButton label="搜索或输入命令" title="搜索或输入命令 (Ctrl+P)" icon={Search} onClick={() => openPalette("all")} />
        <RailButton label={collapsed ? "展开侧栏" : "折叠侧栏"} title={collapsed ? "展开侧栏 (Ctrl+B)" : "折叠侧栏 (Ctrl+B)"} icon={PanelLeft} onClick={props.onToggleSidebar} />
        <RailButton
          label="切换底部面板"
          title="底部面板(Ctrl+J)"
          icon={PanelBottom}
          pressed={state.panel.visible}
          active={state.panel.visible}
          badge={pendingNotices}
          onClick={props.onTogglePanel}
        />
        <RailButton label="CONFIG" title="设置" icon={Settings} onClick={() => openEditor({ kind: "config" })} />
        <RailButton label={`色彩主题:${theme.label},点击切换`} title={`色彩主题:${theme.label}(点击切换)`} icon={theme.icon} onClick={cycleThemeMode} />
        <RailButton label="退出登录" title="退出登录" icon={LogOut} onClick={props.onLogout} />
        {isTauri && <WindowControls vertical />}
      </div>
    </nav>
  );
}
