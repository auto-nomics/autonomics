import { useSyncExternalStore } from "react";

let slot: HTMLDivElement | null = null;
const listeners = new Set<() => void>();
export function setProjectSidebarSlot(node: HTMLDivElement | null) {
  if (slot === node) return;
  slot = node;
  listeners.forEach((listener) => listener());
}
export function useProjectSidebarSlot() {
  return useSyncExternalStore(
    (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    () => slot,
    () => null,
  );
}
