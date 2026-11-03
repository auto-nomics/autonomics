/** Editor grid with stable portal hosts, so moving panes keeps editor state. */

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { editorDrag, setEditorDrag, useEditorDrag } from "../editor-drag";
import { useWorkbenchRuntime } from "../runtime-context";
import { BookOpen, Library, Settings } from "lucide-react";
import { cn } from "@/lib/utils";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import type { Layout } from "react-resizable-panels";
import { editorKind } from "../editor-registry";
import {
  openEditor,
  moveEditor,
  setEditorLayoutRatio,
  setActiveGroup,
  getGroup,
  allGroups,
  getWorkbenchState,
  splitEditorAt,
  moveGroupAt,
  mergeGroup,
  useWorkbenchState,
  type EditorGroup,
  type EditorRef,
  type GroupId,
} from "../workbench-store";
import { EditorTabs } from "./editor-tabs";
import type { EditorLayout, DropDirection } from "../editor-layout";

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
type ScrollPosition = { element: HTMLElement; top: number; left: number };
type HostRegistry = { zones: Map<GroupId, HTMLDivElement>; hosts: Map<string, HTMLDivElement>; scrollPositions: Map<string, ScrollPosition[]> };
function captureScroll(host: HTMLDivElement): ScrollPosition[] {
  return Array.from(host.querySelectorAll<HTMLElement>('[data-slot="scroll-area-viewport"]'), element => ({ element, top: element.scrollTop, left: element.scrollLeft }));
}
/** Reparenting an element can reset descendant scroll offsets in browsers. */
function placeHost(zone: HTMLDivElement, host: HTMLDivElement, id: string, registry: HostRegistry) {
  if (host.parentElement === zone) return;
  const positions = registry.scrollPositions.get(id) ?? captureScroll(host);
  registry.scrollPositions.delete(id);
  zone.appendChild(host);
  for (const { element, top, left } of positions) { element.scrollTop = top; element.scrollLeft = left; }
}
function EditorGroupPane(props: { groupId: GroupId; group: EditorGroup; registry: HostRegistry }) {
  const dragging = useEditorDrag();
  const [dropZone, setDropZone] = useState<"center" | DropDirection | null>(null);
  const runtime = useWorkbenchRuntime();
  const workbench = useWorkbenchState();
  const setZone = useCallback((node: HTMLDivElement | null) => {
    if (!node) { props.registry.zones.delete(props.groupId); return; }
    props.registry.zones.set(props.groupId, node);
    const group = getGroup(getWorkbenchState().groups, props.groupId);
    for (const editor of group?.editors ?? []) {
      const host = props.registry.hosts.get(editor.id);
      if (host) placeHost(node, host, editor.id, props.registry);
    }
  }, [props.groupId, props.registry]);
  useLayoutEffect(() => {
    const zone = props.registry.zones.get(props.groupId);
    return () => {
      for (const [id, host] of props.registry.hosts) {
        if (host.parentElement === zone) props.registry.scrollPositions.set(id, captureScroll(host));
      }
    };
  }, [props.groupId, props.registry]);
  const activeId = props.group.activeId;
  const activeEditor = props.group.editors.find((editor) => editor.id === activeId) ?? null;
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-1 flex-col" data-editor-group={props.groupId} onPointerDownCapture={() => {
      if (getWorkbenchState().groups.active === props.groupId) return;
      setActiveGroup(props.groupId);
      const active = props.group.editors.find(editor => editor.id === props.group.activeId);
      if (active?.kind === "agent") runtime.selectAgent(String(active.params.path ?? ""));
    }}>
      <EditorTabs groupId={props.groupId} group={props.group} />
      <div
        className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
        ref={setZone}
        data-editor-zone={props.groupId}
        onDragOver={(event) => {
          if (!editorDrag) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          const rect = event.currentTarget.getBoundingClientRect();
          const x = (event.clientX - rect.left) / rect.width, y = (event.clientY - rect.top) / rect.height;
          const nearest = Math.min(x, 1 - x, y, 1 - y);
          setDropZone(nearest >= 0.25 ? "center" : nearest === x ? "left" : nearest === 1 - x ? "right" : nearest === y ? "top" : "bottom");
        }}
        onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropZone(null); }}
        onDrop={(event) => {
          if (!editorDrag) return;
          event.preventDefault();
          const dragged = getGroup(workbench.groups, editorDrag.groupId)?.editors.find((item) => item.id === editorDrag!.id);
          if (!editorDrag.id) {
            if (dropZone && dropZone !== "center") moveGroupAt(editorDrag.groupId, props.groupId, dropZone);
            else mergeGroup(editorDrag.groupId, props.groupId, props.group.editors.length);
          } else if (dropZone && dropZone !== "center") splitEditorAt(editorDrag.groupId, editorDrag.id, props.groupId, dropZone);
          else moveEditor(editorDrag.groupId, editorDrag.id, props.groupId, props.group.editors.length);
          if (dragged?.kind === "agent") runtime.selectAgent(String(dragged.params.path ?? ""));
          setEditorDrag(null);
          setDropZone(null);
        }}
      >
        {dragging && <div aria-hidden className="absolute inset-0 z-10" />}
        {dropZone && <div aria-hidden className={cn("pointer-events-none absolute z-20 border-2 border-primary bg-primary/15", dropZone === "right" ? "inset-y-0 right-0 w-1/2" : dropZone === "left" ? "inset-y-0 left-0 w-1/2" : dropZone === "top" ? "inset-x-0 top-0 h-1/2" : dropZone === "bottom" ? "inset-x-0 bottom-0 h-1/2" : "inset-0")} />}
        {!activeEditor && <EditorEmptyState />}
      </div>
    </div>
  );
}

function LayoutNode({ node, path, registry }: { node: EditorLayout; path: string; registry: HostRegistry }) {
  const state = useWorkbenchState();
  if ("group" in node) {
    const group = getGroup(state.groups, node.group);
    return group ? <EditorGroupPane groupId={node.group} group={group} registry={registry} /> : null;
  }
  const first = `layout-${path || "root"}-first`, second = `layout-${path || "root"}-second`;
  const initial: Layout = { [first]: node.ratio, [second]: 100 - node.ratio };
  return <ResizablePanelGroup orientation={node.axis} defaultLayout={initial} onLayoutChanged={layout => {
    const ratio = layout[first];
    if (typeof ratio === "number" && Math.abs(ratio - node.ratio) > 0.1) setEditorLayoutRatio(path, ratio);
  }} className="min-h-0 min-w-0 flex-1">
    <ResizablePanel id={first} minSize="15%" className="flex min-h-0 min-w-0"><LayoutNode node={node.first} path={`${path}0`} registry={registry} /></ResizablePanel>
    <ResizableHandle aria-label={node.axis === "horizontal" ? "调整分屏宽度" : "调整分屏高度"} className={node.axis === "vertical" ? "h-px w-full after:-inset-x-2 after:-top-1 after:h-2.5 after:w-auto" : undefined} />
    <ResizablePanel id={second} minSize="15%" className="flex min-h-0 min-w-0"><LayoutNode node={node.second} path={`${path}1`} registry={registry} /></ResizablePanel>
  </ResizablePanelGroup>;
}
/** The portal target is stable even when its DOM host moves between panes. */
function EditorHost({ editor, groupId, active, registry }: { editor: EditorRef; groupId: GroupId; active: boolean; registry: HostRegistry }) {
  const runtime = useWorkbenchRuntime();
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = "absolute inset-0 min-h-0 min-w-0";
    return element;
  });
  useLayoutEffect(() => {
    registry.hosts.set(editor.id, host);
    const zone = registry.zones.get(groupId);
    if (zone) placeHost(zone, host, editor.id, registry);
    host.style.pointerEvents = active ? "" : "none";
  }, [groupId, host, registry, active, editor.id]);
  useLayoutEffect(() => {
    const focusGroup = () => {
      if (getWorkbenchState().groups.active === groupId) return;
      setActiveGroup(groupId);
      const group = getGroup(getWorkbenchState().groups, groupId);
      const selected = group?.editors.find(item => item.id === group.activeId);
      if (selected?.kind === "agent") runtime.selectAgent(String(selected.params.path ?? ""));
    };
    host.addEventListener("pointerdown", focusGroup, true);
    host.addEventListener("focusin", focusGroup, true);
    return () => { host.removeEventListener("pointerdown", focusGroup, true); host.removeEventListener("focusin", focusGroup, true); };
  }, [groupId, host, runtime]);
  useLayoutEffect(() => () => { host.remove(); registry.hosts.delete(editor.id); registry.scrollPositions.delete(editor.id); }, [host, editor.id, registry]);
  const kind = editorKind(editor.kind);
  if (!kind || (kind.mount === "ephemeral" && !active)) return null;
  return createPortal(<EditorBody editor={editor} active={active} />, host);
}
export function EditorArea() {
  const state = useWorkbenchState();
  const registry = useRef<HostRegistry>({ zones: new Map(), hosts: new Map(), scrollPositions: new Map() }).current;
  const editors = allGroups(state.groups).flatMap(([groupId, group]) => group.editors.map(editor => ({ editor, groupId, active: editor.id === group.activeId })));
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background">
    <LayoutNode node={state.editorLayout} path="" registry={registry} />
    {editors.map(item => <EditorHost key={item.editor.id} {...item} registry={registry} />)}
  </div>;
}
