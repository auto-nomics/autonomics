import { Camera, Compass, Library, Rss, type LucideIcon } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { setProjectSidebarSlot } from "@/features/projects/sidebar-slot";
import { setBibliographySidebarSlot } from "@/features/bibliography/sidebar-slot";
import { WikiSidebarView } from "@/features/wiki/wiki-sidebar-view";
import { VfsSidebar } from "@/features/vfs";
import { DeckSidebarView } from "@/features/deck/deck-sidebar-view";
import { Button } from "@/components/ui/button";
import { AgentSidebar } from "./parts/agent-sidebar";
import { openEditor, useWorkbenchState } from "./workbench-store";
import { openBibliographyEditor } from "./static-commands";
import { useActiveEditorContext } from "./active-editor-context";
import { registerWorkbenchView, type WorkbenchViewProps } from "./view-registry";

const BIB_VIEWS: readonly { id: BibliographyWorkspace; label: string; name: string; icon: LucideIcon }[] = [
  { id: "library", label: "文献库", name: "Bibliography library", icon: Library },
  { id: "discover", label: "发现", name: "Discover", icon: Compass },
  { id: "snapshots", label: "快照", name: "Snapshots", icon: Camera },
  { id: "subscriptions", label: "订阅", name: "Subscriptions", icon: Rss },
];

function BibliographyView({ mobile }: WorkbenchViewProps) {
  const state = useWorkbenchState();

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
registerWorkbenchView({ id: "bibliography", title: "文献与证据", defaultLocation: "primarySidebar", body: BibliographyView });

function ProjectsView() {
  const { editor: active } = useActiveEditorContext();

    return <><div ref={setProjectSidebarSlot} className="flex min-h-0 flex-1 flex-col" />{active?.kind !== "plugin-projects" && <Button variant="ghost" onClick={() => openEditor({ kind: "plugin-projects" })}>打开插件项目</Button>}</>;

}
registerWorkbenchView({ id: "projects", title: "插件项目", defaultLocation: "primarySidebar", body: ProjectsView });

function WikiView() {

    return (
      <div role="group" aria-label="知识库工作区" className="flex min-h-0 flex-1 flex-col">
        <WikiSidebarView onOpenGraph={() => openEditor({ kind: "wiki-graph" })} />
      </div>
    );

}
registerWorkbenchView({ id: "wiki", title: "知识库", defaultLocation: "primarySidebar", body: WikiView });

function VfsView() {

    return <VfsSidebar />;

}
registerWorkbenchView({ id: "vfs", title: "文件", defaultLocation: "primarySidebar", body: VfsView });

function DecksView() {

    return (
      <div role="group" aria-label="幻灯片工作区" className="flex min-h-0 flex-1 flex-col">
        <DeckSidebarView />
      </div>
    );

}
registerWorkbenchView({ id: "decks", title: "幻灯片", defaultLocation: "primarySidebar", body: DecksView });

registerWorkbenchView({ id: "agents", title: "Agent", defaultLocation: "primarySidebar", body: ({ onSpawn }) => <AgentSidebar onSpawn={onSpawn} /> });
