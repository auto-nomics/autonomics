import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { WorkbenchViewSlot } from "../view-host";
import { startViewDrag, viewDropTarget } from "../view-drag";
import { useWorkbenchViews } from "../view-registry";
import { selectPanelView, useViewLayout, workbenchViewLocation } from "../view-layout";
import { setPanelTab, setPanelVisible, useWorkbenchState, type PanelTab } from "../workbench-store";
import "../panel-views";
import { ViewLocationMenu } from "../view-location-menu";

export function BottomPanel() {
  const state = useWorkbenchState();
  const layout = useViewLayout();
  const views = useWorkbenchViews().filter(view => workbenchViewLocation(view.id, layout) === "panel");
  const selected = views.find(view => view.id === (layout.activePanel ?? state.panel.tab)) ?? views[0];
  return <section {...viewDropTarget("panel")} aria-label="底部面板" className="flex h-full min-h-0 flex-col bg-background">
    <div className="flex h-8 shrink-0 items-center gap-1 border-b px-1.5" role="tablist" aria-label="面板标签">
      {views.map(view => <button key={view.id} draggable onDragStart={event => startViewDrag(event, view.id)} type="button" role="tab" aria-label={view.title} aria-selected={selected?.id === view.id} className={cn("rounded-md px-2 py-0.5 text-xs", selected?.id === view.id ? "bg-accent text-foreground" : "text-muted-foreground")} onClick={() => {
        if (["events", "notices", "terminal"].includes(view.id)) { selectPanelView(null); setPanelTab(view.id as PanelTab); }
        else selectPanelView(view.id);
      }}>{view.title}{view.id === "notices" && state.panel.unreadNotices > 0 && <span className="ml-1">{state.panel.unreadNotices}</span>}</button>)}
      <span className="flex-1" />
      {selected && <ViewLocationMenu id={selected.id} />}
      <Button size="icon-xs" variant="ghost" className="ml-auto" aria-label="收起面板" onClick={() => setPanelVisible(false)}><X /></Button>
    </div>
    {selected && <WorkbenchViewSlot key={selected.id} id={selected.id} />}
  </section>;
}
