/**
 * Editor tab strip for one editor group. Tabs carry the registered kind's
 * title/icon; closing consults the kind's confirmLeave veto (used from M2
 * by unsaved wiki edits).
 */

import { X } from "lucide-react";
import { SplitSquareHorizontal, Columns2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { editorKind } from "../editor-registry";
import { activateEditor, closeEditor, closeSplit, splitEditor, useWorkbenchState, type EditorGroup, type GroupId } from "../workbench-store";

export function EditorTabs(props: { groupId: GroupId; group: EditorGroup }) {
  const state = useWorkbenchState();
  const split = state.groups.secondary !== null;
  const isMain = props.groupId === "main";

  return (
    <div
      role="tablist"
      aria-label={isMain ? "编辑器标签" : "分屏编辑器标签"}
      className="flex h-9 shrink-0 items-stretch border-b bg-muted/40"
    >
      {props.group.editors.map((editor) => {
        const kind = editorKind(editor.kind);
        if (!kind) return null;
        const active = editor.id === props.group.activeId;
        const Icon = kind.icon?.(editor);
        const title = kind.title(editor);
        return (
          // VSCode-shaped tab: flat cells on the strip; the active tab
          // lifts to the background color with a 2px accent on top.
          <div
            key={editor.id}
            role="tab"
            aria-selected={active}
            aria-label={title}
            title={title}
            onClick={() => activateEditor(props.groupId, editor.id)}
            className={cn(
              "group/tab relative flex min-w-0 max-w-48 shrink-0 cursor-pointer items-center gap-1.5 border-r px-2.5 text-xs",
              active
                ? "bg-background text-foreground shadow-[inset_0_2px_0_0] shadow-primary"
                : "text-muted-foreground hover:bg-accent/60",
            )}
          >
            {Icon && <Icon className="size-3.5 shrink-0" aria-hidden />}
            <span className="min-w-0 truncate">{title}</span>
            <button
              type="button"
              aria-label={`关闭 ${title}`}
              title={`关闭 ${title}`}
              disabled={editor.pinned}
              onClick={(event) => {
                event.stopPropagation();
                if (kind.confirmLeave?.(editor)) return; // vetoed (unsaved edits)
                closeEditor(editor.id);
              }}
              className="rounded-sm p-0.5 opacity-0 transition hover:bg-accent focus-visible:opacity-100 group-hover/tab:opacity-100 disabled:opacity-30"
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
      <div className="ml-auto flex items-center gap-0.5 px-1.5">
        {isMain && !split && (
          <button
            type="button"
            aria-label="拆分编辑器"
            title="拆分编辑器"
            disabled={!props.group.activeId}
            onClick={splitEditor}
            className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
          >
            <SplitSquareHorizontal className="size-3.5" />
          </button>
        )}
        {!isMain && (
          <button
            type="button"
            aria-label="关闭分屏"
            title="关闭分屏"
            onClick={closeSplit}
            className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <Columns2 className="size-3.5" />
          </button>
        )}
      </div>
    </div>
  );
}
