import { WorkbenchViewSlot } from "../view-host";
import { startViewDrag, viewDropTarget } from "../view-drag";
import { Button } from "@/components/ui/button";
import { useWorkbenchViews } from "../view-registry";
import { setSecondarySidebarWidth, toggleSecondarySidebar, useViewLayout, workbenchViewLocation } from "../view-layout";
import { ViewLocationMenu } from "../view-location-menu";

export function SecondarySidebar() {
  const layout = useViewLayout();
  const views = useWorkbenchViews().filter(view => workbenchViewLocation(view.id, layout) === "secondarySidebar");
  if (!views.length) return null;
  return <aside {...viewDropTarget("secondarySidebar")} aria-label="辅助侧边栏" hidden={!layout.secondaryVisible} style={{ width: layout.secondaryWidth, display: layout.secondaryVisible ? undefined : "none" }} className="relative flex min-w-48 shrink-0 flex-col border-l bg-background">
    <div role="separator" aria-label="调整辅助侧边栏宽度" aria-orientation="vertical" aria-valuemin={192} aria-valuemax={640} aria-valuenow={layout.secondaryWidth} tabIndex={0}
      className="absolute inset-y-0 -left-1 z-30 w-2 cursor-col-resize focus-visible:bg-primary/30"
      onPointerDown={event => { event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); }}
      onPointerMove={event => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
        const edge = event.currentTarget.parentElement?.getBoundingClientRect().right;
        if (edge !== undefined) setSecondarySidebarWidth(edge - event.clientX);
      }}
      onKeyDown={event => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        setSecondarySidebarWidth(layout.secondaryWidth + (event.key === "ArrowLeft" ? 10 : -10));
      }} />
    <Button size="xs" variant="ghost" onClick={toggleSecondarySidebar}>隐藏辅助侧边栏</Button>
    <div className="flex min-h-0 flex-1 flex-col overflow-auto">
    {views.map(view => {
      return <section key={view.id} aria-label={view.title} className="flex min-h-48 flex-1 flex-col overflow-hidden">
        <div className="flex shrink-0 flex-wrap items-center gap-1 border-b p-1 text-xs">
          <span draggable onDragStart={event => startViewDrag(event, view.id)} className="mr-auto px-1">{view.title}</span>
          <ViewLocationMenu id={view.id} />
        </div>
        <WorkbenchViewSlot id={view.id} />
      </section>;
    })}
    </div>
  </aside>;
}
