import { useTranslation } from "react-i18next";
import i18n from "@/i18n";
import { WikiLocalGraph } from "./wiki-local-graph";
import { CanvasSidebar } from "@/features/canvas/canvas-sidebar";
import { PencilRuler } from "lucide-react";
import { Compass, Library, FileText, Rss, type LucideIcon } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { setProjectSidebarSlot } from "@/features/projects/sidebar-slot";
import { setBibliographySidebarSlot } from "@/features/bibliography/sidebar-slot";
import { WikiSidebarView } from "@/features/wiki/wiki-sidebar-view";
import { VfsSidebar } from "@/features/vfs";
import { DeckSidebarView } from "@/features/deck/deck-sidebar-view";
import { Button } from "@/components/ui/button";
import { AgentSidebar } from "./parts/agent-sidebar";
import { openEditor, setContainerView, useWorkbenchState } from "./workbench-store";
import { openBibliographyEditor } from "./static-commands";
import { useActiveEditorContext } from "./active-editor-context";
import { registerWorkbenchView, type WorkbenchViewProps } from "./view-registry";

const BIB_VIEWS: readonly { id: BibliographyWorkspace; icon: LucideIcon }[] = [
  { id: "library", icon: Library },
  { id: "manuscripts", icon: FileText },
  { id: "discover", icon: Compass },
  { id: "subscriptions", icon: Rss },
];

function BibliographyView({ mobile }: WorkbenchViewProps) {
  const { t } = useTranslation("workbench");
  const state = useWorkbenchState();
  const active = state.containerViews.bibliography ?? "library";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {mobile && <div role="group" aria-label={t("bibViews.workspaceAria")} className="flex shrink-0 flex-col gap-0.5 p-2">
        {BIB_VIEWS.map((view) => (
          <Button
            key={view.id}
            size="sm"
            variant={active === view.id ? "secondary" : "ghost"}
            className="w-full shrink-0 justify-start gap-2 px-2"
            aria-label={t(`bibViews.${view.id}`)}
            aria-current={active === view.id ? "page" : undefined}
            title={t(`bibViews.${view.id}`)}
            onClick={() => openBibliographyEditor(view.id)}
          >
            <view.icon />
            <span className="truncate text-xs">{t(`bibViews.${view.id}`)}</span>
          </Button>
        ))}
      </div>}
      <div ref={setBibliographySidebarSlot} className="flex min-h-0 flex-1 flex-col" />
    </div>
  );
}

function ProjectsView() {
  const { t } = useTranslation("workbench");
  const { editor: active } = useActiveEditorContext();

    return <><div ref={setProjectSidebarSlot} className="flex min-h-0 flex-1 flex-col" />{active?.kind !== "plugin-projects" && <Button variant="ghost" onClick={() => openEditor({ kind: "plugin-projects" })}>{t("sidebar.plugins")}</Button>}</>;

}

function WikiView() {
  const mode = useWorkbenchState().containerViews.wiki;

    return (
      <div role="group" aria-label={i18n.t("workbench:views.wiki")} className="flex min-h-0 flex-1 flex-col">
        <WikiSidebarView graph={<WikiLocalGraph />} graphActive={mode === "graph"} onOpenGraph={() => setContainerView("wiki", "graph")} onCloseGraph={() => setContainerView("wiki", "browse")} />
      </div>
    );

}

function VfsView() {

    return <VfsSidebar />;

}

function DecksView() {

    return (
      <div role="group" aria-label={i18n.t("workbench:views.decks")} className="flex min-h-0 flex-1 flex-col">
        <DeckSidebarView />
      </div>
    );

}

function registerBuiltinViewTitles(): void {
registerWorkbenchView({ id: "bibliography", title: i18n.t("workbench:views.bibliography"), defaultLocation: "primarySidebar", body: BibliographyView });
registerWorkbenchView({ id: "projects", title: i18n.t("workbench:views.projects"), defaultLocation: "primarySidebar", body: ProjectsView });
registerWorkbenchView({ id: "wiki", title: i18n.t("workbench:views.wiki"), defaultLocation: "primarySidebar", body: WikiView });
registerWorkbenchView({ id: "vfs", title: i18n.t("workbench:views.vfs"), defaultLocation: "primarySidebar", body: VfsView });
registerWorkbenchView({ id: "decks", title: i18n.t("workbench:views.decks"), defaultLocation: "primarySidebar", body: DecksView });
registerWorkbenchView({ id: "agents", title: i18n.t("workbench:views.agents"), defaultLocation: "primarySidebar", body: ({ onSpawn }) => <AgentSidebar onSpawn={onSpawn} /> });
registerWorkbenchView({ id: "canvases", title: i18n.t("workbench:views.canvases"), icon: PencilRuler, defaultLocation: "primarySidebar", body: CanvasSidebar });
}

registerBuiltinViewTitles();
// Re-register on language change so view titles follow the active locale.
i18n.on("languageChanged", registerBuiltinViewTitles);
