import { WikiGraph } from "@/features/wiki/wiki-graph";
import { allGroups, getGroup, useWorkbenchState, type WikiEntryEditorState } from "./workbench-store";

export function WikiLocalGraph() {
  const state = useWorkbenchState();
  const group = getGroup(state.groups, state.groups.active);
  const current = group?.editors.find(item => item.id === group.activeId && item.kind === "wiki-entry");
  const last = allGroups(state.groups).flatMap(([, group]) => group.editors).find(item => item.kind === "wiki-entry" && item.id === state.lastWikiEditorId);
  const editor = current ?? last;
  const wikiState = editor?.state as WikiEntryEditorState | null | undefined;
  const location = wikiState?.stack?.[wikiState.cursor];
  const entryId = location?.entryId ?? (editor ? String(editor.params.entryId ?? "") : "");
  return <WikiGraph entryId={entryId} title={location?.title || wikiState?.title || ""} />;
}
