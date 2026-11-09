import type { DragEvent } from "react";
import { workbenchView, type ViewLocation } from "./view-registry";
import { moveWorkbenchView } from "./view-layout";
import { setContainer, setPanelVisible, setSidebarCollapsed } from "./workbench-store";

const mime = "application/x-autonomics-view";
export function startViewDrag(event: DragEvent, id: string) {
  event.dataTransfer.setData(mime, id);
  event.dataTransfer.effectAllowed = "move";
}
export function viewDropTarget(location: ViewLocation) {
  return {
    "data-view-drop-location": location,
    onDragOver(event: DragEvent) {
      if (!event.dataTransfer.types.includes(mime)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
    },
    onDrop(event: DragEvent) {
      const id = event.dataTransfer.getData(mime);
      if (!workbenchView(id)) return;
      event.preventDefault(); event.stopPropagation();
      moveWorkbenchView(id, location);
      if (location === "panel") setPanelVisible(true);
      if (location === "primarySidebar") {
        setContainer(id);
        setSidebarCollapsed(false);
      }
    },
  };
}

// Portal bodies bubble through their React owner, so DOM drop regions also
// receive native capture events independently of the component hierarchy.
for (const type of ["dragover", "drop"] as const) document.addEventListener(type, event => {
  if (!(event.target instanceof Element)) return;
  const region = event.target.closest<HTMLElement>("[data-view-drop-location]");
  const location = region?.dataset.viewDropLocation as ViewLocation | undefined;
  if (!location || !event.dataTransfer?.types.includes(mime)) return;
  const handlers = viewDropTarget(location);
  if (type === "dragover") handlers.onDragOver(event as unknown as DragEvent);
  else handlers.onDrop(event as unknown as DragEvent);
}, true);
