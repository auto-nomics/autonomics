import { useSyncExternalStore } from "react";
/** Resource-level lifecycle guards; independent of editor instances. */
const dirty = new Set<string>();
let snapshot: string[] = [];
const listeners = new Set<() => void>();
function guardUnload(event: BeforeUnloadEvent) {
  if (!dirty.size) return;
  event.preventDefault();
  event.returnValue = "";
}

export function setDocumentDirty(resource: string, value: boolean) {
  if (dirty.has(resource) === value) return;
  const hadDirty = dirty.size > 0;
  if (value) dirty.add(resource); else dirty.delete(resource);
  if (!hadDirty && dirty.size) window.addEventListener("beforeunload", guardUnload);
  if (hadDirty && !dirty.size) window.removeEventListener("beforeunload", guardUnload);
  snapshot = [...dirty];
  for (const listener of listeners) listener();
}

export function dirtyDocumentResources(): string[] { return [...dirty]; }
export function useDirtyDocuments() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => snapshot);
}
