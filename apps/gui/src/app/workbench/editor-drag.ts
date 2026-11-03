import type { EditorId, GroupId } from "./workbench-store";

// Only drags started by this workbench may change its editor layout.
export let editorDrag: { groupId: GroupId; id: EditorId } | null = null;
export function setEditorDrag(value: typeof editorDrag) { editorDrag = value; }
