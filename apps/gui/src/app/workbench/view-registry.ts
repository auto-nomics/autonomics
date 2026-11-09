import { useSyncExternalStore, type ComponentType } from "react";
import type { LucideIcon } from "lucide-react";

export type ViewLocation = "primarySidebar" | "secondarySidebar" | "panel";
export type WorkbenchViewProps = { mobile: boolean; onSpawn: () => void };
export type WorkbenchView = {
  id: string;
  title: string;
  icon?: LucideIcon;
  defaultLocation: ViewLocation;
  body: ComponentType<WorkbenchViewProps>;
};

const views = new Map<string, WorkbenchView>();
let snapshot: WorkbenchView[] = [];
const listeners = new Set<() => void>();

/** Contributions describe content; the workbench owns its placement. */
export function registerWorkbenchView(view: WorkbenchView): () => void {
  views.set(view.id, view);
  snapshot = [...views.values()];
  for (const listener of listeners) listener();
  return () => {
    // An old contribution must not remove its replacement.
    if (views.get(view.id) !== view) return;
    views.delete(view.id);
    snapshot = [...views.values()];
    for (const listener of listeners) listener();
  };
}

export function workbenchView(id: string): WorkbenchView | undefined {
  return views.get(id);
}

export function allWorkbenchViews(): WorkbenchView[] {
  return snapshot;
}
export function useWorkbenchViews(): WorkbenchView[] {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, allWorkbenchViews);
}
