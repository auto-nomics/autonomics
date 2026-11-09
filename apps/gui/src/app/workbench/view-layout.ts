import { useSyncExternalStore } from "react";
import { allWorkbenchViews, workbenchView, type ViewLocation } from "./view-registry";
import { setContainer, setSidebarCollapsed, type WorkbenchState } from "./workbench-store";

const key = "autonomics.workbench.views.v1";
type Layout = { locations: Record<string, ViewLocation>; activePanel: string | null; secondaryWidth: number; secondaryVisible: boolean };
let state: Layout = { locations: {}, activePanel: null, secondaryWidth: 288, secondaryVisible: true };
try {
  const saved = JSON.parse(localStorage.getItem(key) ?? "null");
  if (typeof saved?.secondaryVisible === "boolean") state.secondaryVisible = saved.secondaryVisible;
  if (Number.isFinite(saved?.secondaryWidth)) state.secondaryWidth = Math.max(192, Math.min(640, saved.secondaryWidth));
  if (saved && typeof saved.locations === "object" && saved.locations) {
    for (const [id, location] of Object.entries(saved.locations)) {
      if (location === "primarySidebar" || location === "secondarySidebar" || location === "panel") state.locations[id] = location;
    }
    if (typeof saved.activePanel === "string" && state.locations[saved.activePanel] === "panel") state.activePanel = saved.activePanel;
  }
} catch { /* Restore defaults if storage is unavailable or invalid. */ }
const listeners = new Set<() => void>();
export function workbenchViewLocation(id: string, layout: Layout): ViewLocation {
  return layout.locations[id] ?? workbenchView(id)?.defaultLocation ?? "primarySidebar";
}
export function revealWorkbenchView(id: string) {
  moveWorkbenchView(id, workbenchViewLocation(id, state));
}
export function isWorkbenchViewVisible(id: string, workbench: WorkbenchState, layout: Layout, mobile: boolean) {
  const location = workbenchViewLocation(id, layout);
  if (location === "primarySidebar") return !workbench.sidebar.collapsed && workbench.sidebar.container === id;
  if (location === "secondarySidebar") return !mobile && layout.secondaryVisible;
  const views = allWorkbenchViews().filter(view => workbenchViewLocation(view.id, layout) === "panel");
  const selected = views.find(view => view.id === (layout.activePanel ?? workbench.panel.tab)) ?? views[0];
  return workbench.panel.visible && selected?.id === id;
}
export function moveWorkbenchView(id: string, location: ViewLocation) {
  state = { ...state, secondaryVisible: location === "secondarySidebar" ? true : state.secondaryVisible, locations: { ...state.locations, [id]: location }, activePanel: location === "panel" ? id : state.activePanel === id ? null : state.activePanel };
  try { localStorage.setItem(key, JSON.stringify(state)); } catch { /* Session only. */ }
  for (const listener of listeners) listener();
  if (location === "primarySidebar") { setContainer(id); setSidebarCollapsed(false); }
}
export function selectPanelView(id: string | null) {
  state = { ...state, activePanel: id };
  try { localStorage.setItem(key, JSON.stringify(state)); } catch { /* Session only. */ }
  for (const listener of listeners) listener();
}
export function setSecondarySidebarWidth(width: number) {
  if (!Number.isFinite(width)) return;
  state = { ...state, secondaryWidth: Math.max(192, Math.min(640, width)) };
  try { localStorage.setItem(key, JSON.stringify(state)); } catch { /* Session only. */ }
  for (const listener of listeners) listener();
}
export function toggleSecondarySidebar() {
  state = { ...state, secondaryVisible: !state.secondaryVisible };
  try { localStorage.setItem(key, JSON.stringify(state)); } catch { /* Session only. */ }
  for (const listener of listeners) listener();
}
export function resetWorkbenchViewLocations() {
  state = { locations: {}, activePanel: null, secondaryWidth: 288, secondaryVisible: true };
  try { localStorage.removeItem(key); } catch { /* Session only. */ }
  for (const listener of listeners) listener();
}
export function useViewLayout() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => state);
}
