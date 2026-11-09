import { editorKind } from "./editor-registry";
import { getGroup, useWorkbenchState, type WorkbenchState } from "./workbench-store";

/** Workbench focus is independent of which sidebar or panel has DOM focus.
 * Views consume this context instead of each interpreting group state. */
export function activeEditorContext(state: WorkbenchState) {
  const groupId = state.groups.active;
  const group = getGroup(state.groups, groupId);
  const editor = group?.editors.find(item => item.id === group.activeId) ?? null;
  return {
    groupId,
    editor,
    kind: editor ? editorKind(editor.kind) : undefined,
  };
}

export function useActiveEditorContext() {
  return activeEditorContext(useWorkbenchState());
}
