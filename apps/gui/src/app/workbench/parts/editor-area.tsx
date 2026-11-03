/**
 * Editor area: one or two editor groups (split), each pane hosting its
 * group's editors.
 *
 * Mounting model — keep-alive editor bodies render as children of their
 * group pane and only toggle `hidden` when inactive, so React state (wiki
 * drafts, virtualized lists, hosted readers) survives tab switches exactly
 * like the old shell's hidden sections. Group restructuring (split, moving
 * an editor across groups) remounts the affected bodies — rare operations,
 * accepted for now. Ephemeral kinds (readers, from M3) mount only while
 * active; their persisted state lives outside React.
 */

import { useState } from "react";
import { editorDrag, setEditorDrag } from "../editor-drag";
import { useWorkbenchRuntime } from "../runtime-context";
import { BookOpen, Library, Settings } from "lucide-react";
import { cn } from "@/lib/utils";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import type { Layout } from "react-resizable-panels";
import { editorKind } from "../editor-registry";
import {
  openEditor,
  moveEditor,
  setEditorSplitRatio,
  useWorkbenchState,
  type EditorGroup,
  type EditorRef,
  type GroupId,
} from "../workbench-store";
import { EditorTabs } from "./editor-tabs";

function EditorBody(props: { editor: EditorRef; active: boolean }) {
  const kind = editorKind(props.editor.kind);
  if (!kind) return null;
  const Body = kind.body;
  const keepAlive = kind.mount === "keep-alive";
  const active = props.active;
  // Keep-alive bodies stay mounted; occlusion mode decides how they hide.
  // "display" (hidden attribute) unmounts layout; "visibility" keeps the
  // subtree laid out so scroll positions / canvas state survive switches.
  const occluded = keepAlive && kind.hideMode === "visibility" && !active;
  const displayHidden = keepAlive && kind.hideMode !== "visibility" ? !active : undefined;
  return (
    <div
      hidden={displayHidden}
      aria-hidden={keepAlive && !active ? true : undefined}
      inert={occluded || undefined}
      className={cn(
        "absolute inset-0 flex min-h-0 min-w-0 flex-col",
        occluded && "editor-body-occluded",
        keepAlive && !active && kind.hideMode !== "visibility" && "pointer-events-none",
      )}
    >
      <Body editor={props.editor} />
    </div>
  );
}

function EditorEmptyState() {
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-4 p-8 text-center">
      <div className="text-sm text-muted-foreground">按 <kbd className="rounded border bg-muted px-1 font-mono text-[11px]">Ctrl P</kbd> 搜索命令,或从左侧打开</div>
      <div className="flex gap-2">
        <button
          type="button"
          className="flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-xs hover:bg-accent"
          onClick={() => openEditor({ kind: "bibliography" })}
        >
          <Library className="size-3.5" />文献库
        </button>
        <button
          type="button"
          className="flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-xs hover:bg-accent"
          onClick={() => openEditor({ kind: "wiki" })}
        >
          <BookOpen className="size-3.5" />知识库
        </button>
        <button
          type="button"
          className="flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-xs hover:bg-accent"
          onClick={() => openEditor({ kind: "config" })}
        >
          <Settings className="size-3.5" />设置
        </button>
      </div>
    </div>
  );
}

/** One group pane: tab strip + the zone hosting every keep-alive body of
 *  the group (hidden unless active) plus the active ephemeral body. */
function EditorGroupPane(props: { groupId: GroupId; group: EditorGroup }) {
  const [dropZone, setDropZone] = useState<"center" | "right" | null>(null);
  const runtime = useWorkbenchRuntime();
  const workbench = useWorkbenchState();
  const activeId = props.group.activeId;
  const activeEditor = props.group.editors.find((editor) => editor.id === activeId) ?? null;
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col" data-editor-group={props.groupId}>
      <EditorTabs groupId={props.groupId} group={props.group} />
      <div
        className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
        data-editor-zone={props.groupId}
        onDragOver={(event) => {
          if (!editorDrag) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          const rect = event.currentTarget.getBoundingClientRect();
          setDropZone(props.groupId === "main" && !workbench.groups.secondary && event.clientX > rect.left + rect.width * 0.75 ? "right" : "center");
        }}
        onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropZone(null); }}
        onDrop={(event) => {
          if (!editorDrag) return;
          event.preventDefault();
          const dragged = workbench.groups[editorDrag.groupId]?.editors.find((item) => item.id === editorDrag!.id);
          const target = dropZone === "right" ? "secondary" : props.groupId;
          moveEditor(editorDrag.groupId, editorDrag.id, target, workbench.groups[target]?.editors.length ?? 0);
          if (dragged?.kind === "agent") runtime.selectAgent(String(dragged.params.path ?? ""));
          setEditorDrag(null);
          setDropZone(null);
        }}
      >
        {dropZone && <div aria-hidden className={cn("pointer-events-none absolute inset-y-0 z-20 border-2 border-primary bg-primary/15", dropZone === "right" ? "right-0 w-1/2" : "inset-x-0")} />}
        {!activeEditor && <EditorEmptyState />}
        {props.group.editors.map((editor) => {
          const kind = editorKind(editor.kind);
          if (!kind) return null;
          if (kind.mount === "ephemeral" && editor.id !== activeId) return null;
          return <EditorBody key={editor.id} editor={editor} active={editor.id === activeId} />;
        })}
      </div>
    </div>
  );
}

export function EditorArea() {
  const state = useWorkbenchState();
  const split = state.groups.secondary !== null;

  const handleSplitLayout = (layout: Layout) => {
    const ratio = layout["ed-main"];
    if (typeof ratio === "number" && ratio >= 25 && ratio <= 75) setEditorSplitRatio(ratio);
  };

  const defaultSplitLayout: Layout | undefined = split
    ? { "ed-main": state.splitRatio, "ed-secondary": 100 - state.splitRatio }
    : undefined;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background">
      {split ? (
        <ResizablePanelGroup
          orientation="horizontal"
          defaultLayout={defaultSplitLayout}
          onLayoutChanged={handleSplitLayout}
          className="min-h-0 min-w-0 flex-1"
        >
          <ResizablePanel id="ed-main" minSize="25%" className="flex min-w-0">
            <EditorGroupPane groupId="main" group={state.groups.main} />
          </ResizablePanel>
          <ResizableHandle aria-label="调整分屏宽度" />
          <ResizablePanel id="ed-secondary" minSize="20%" className="flex min-w-0">
            <EditorGroupPane groupId="secondary" group={state.groups.secondary!} />
          </ResizablePanel>
        </ResizablePanelGroup>
      ) : (
        <EditorGroupPane groupId="main" group={state.groups.main} />
      )}
    </div>
  );
}
