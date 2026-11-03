/**
 * Activity bar: the 48px icon rail. Top = sidebar containers (literature,
 * knowledge base) and the agent auxiliary toggle; bottom = panel toggle,
 * settings, theme, logout. Clicking the active container icon hides the
 * side bar (VSCode behavior); clicking it again reveals it.
 */

import { Bot, BookOpen, Library, PanelBottom, Settings } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { openEditor, setContainer, setSidebarCollapsed, useWorkbenchState, type ContainerId } from "../workbench-store";
import { openBibliographyEditor } from "../static-commands";

function RailButton(props: {
  label: string;
  title: string;
  icon: LucideIcon;
  active?: boolean;
  pressed?: boolean;
  badge?: boolean;
  statusSlot?: React.ReactNode;
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
      {props.statusSlot}
      {props.badge && <span aria-hidden className="absolute right-1.5 top-1.5 size-2 rounded-full bg-primary" />}
    </Button>
  );
}

export function ActivityBar(props: {
  mobile: boolean;
  onToggleSidebar: () => void;
  onToggleAux: () => void;
  onTogglePanel: () => void;
  onLogout: () => void;
}) {
  const state = useWorkbenchState();
  const collapsed = state.sidebar.collapsed;

  const containerClick = (container: ContainerId) => {
    // Only a click on the already-active container WITH its editor open
    // hides the side bar; otherwise the click switches the container,
    // reveals it and opens/focuses the container's editor.
    const kind = container === "bibliography" ? "bibliography" : "wiki";
    const hasEditor = [...state.groups.main.editors, ...(state.groups.secondary?.editors ?? [])]
      .some((editor) => editor.kind === kind);
    if (state.sidebar.container === container && hasEditor && !collapsed) {
      props.onToggleSidebar();
      return;
    }
    setContainer(container);
    setSidebarCollapsed(false);
    if (container === "bibliography") openBibliographyEditor();
    else openEditor({ kind: "wiki" });
  };

  const bibActive = !collapsed && state.sidebar.container === "bibliography";
  const wikiActive = !collapsed && state.sidebar.container === "wiki";
  const running = state.panel.unreadNotices > 0 && (!state.panel.visible || state.panel.tab !== "notices");

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
          title="知识库"
          icon={BookOpen}
          active={wikiActive}
          onClick={() => containerClick("wiki")}
        />
        <RailButton
          label="Agent 副窗"
          title="Agent 对话(Ctrl+Shift+P 搜索「切换」)"
          icon={Bot}
          pressed={state.aux.visible}
          active={state.aux.visible}
          statusSlot={
            <span aria-hidden className={cn("absolute bottom-1 right-1.5 size-1.5 rounded-full", state.aux.visible ? "bg-primary" : "bg-muted-foreground/40")} />
          }
          onClick={props.onToggleAux}
        />
      </div>
      <div className="mt-auto flex flex-col items-center gap-0.5">
        <RailButton
          label="切换底部面板"
          title="底部面板(Ctrl+J)"
          icon={PanelBottom}
          pressed={state.panel.visible}
          active={state.panel.visible}
          badge={running}
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
