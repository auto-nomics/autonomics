/**
 * Activity bar: the 48px icon rail. Top = sidebar containers (literature,
 * knowledge base, agent overview); bottom = panel toggle and settings.
 * Clicking the active container icon hides the side bar (VSCode
 * behavior); clicking it again reveals it.
 */

import { BookOpen, Bot, Library, PanelBottom, Settings } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { setContainer, setSidebarCollapsed, openEditor, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openBibliographyEditor } from "../static-commands";

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
  const collapsed = state.sidebar.collapsed;

  const containerClick = (container: ContainerId) => {
    // Rail container clicks ALWAYS reveal the side bar with that
    // container — collapsing is the title-bar button / Ctrl+B's job. This
    // keeps repeated clicks idempotent (browse lists stay reachable).
    setContainer(container);
    setSidebarCollapsed(false);
    if (container === "bibliography") openBibliographyEditor();
  };

  const bibActive = !collapsed && state.sidebar.container === "bibliography";
  const wikiActive = !collapsed && state.sidebar.container === "wiki";
  const agentsActive = !collapsed && state.sidebar.container === "agents";
  const pendingNotices = state.panel.unreadNotices > 0 && (!state.panel.visible || state.panel.tab !== "notices");

  return (
    <nav
      aria-label="工作台活动栏"
      className={cn("flex w-12 shrink-0 flex-col items-center border-r bg-sidebar py-1 text-sidebar-foreground", props.mobile && "hidden")}
    >
      <div className="flex flex-col items-center gap-0.5">
        <RailButton
          label="Bibliography"
          title="文献与证据"
          icon={Library}
          active={bibActive}
          onClick={() => containerClick("bibliography")}
        />
        <RailButton
          label="Wiki"
          title="知识库(条目即编辑器标签,Ctrl+K 检索)"
          icon={BookOpen}
          active={wikiActive}
          onClick={() => containerClick("wiki")}
        />
        <RailButton
          label="Agent"
          title="Agent 概览(对话即编辑器标签)"
          icon={Bot}
          active={agentsActive}
          onClick={() => containerClick("agents")}
        />
      </div>
      <div className="mt-auto flex flex-col items-center gap-0.5">
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
        {/*
          Theme toggle and logout live in the title bar (and the mobile
          drawer footer); the rail keeps the activity-bar set minimal.
        */}
      </div>
    </nav>
  );
}
