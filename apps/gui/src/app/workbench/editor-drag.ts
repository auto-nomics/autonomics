import { useSyncExternalStore } from "react";
import type { EditorId, GroupId } from "./workbench-store";

// Only drags started by this workbench may change its editor layout.
export let editorDrag: { groupId: GroupId; id: EditorId } | null = null;
const listeners = new Set<() => void>();
export function setEditorDrag(value: typeof editorDrag) {
  editorDrag = value;
  for (const listener of listeners) listener();
}
export function useEditorDrag() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener); }, () => editorDrag);
}
