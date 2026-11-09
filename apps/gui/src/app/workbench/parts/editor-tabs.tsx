/**
 * Editor tab strip for one editor group. Tabs carry the registered kind's
 * title/icon; closing consults the kind's confirmLeave veto. Agent tabs
 * drive the runtime: activating selects the agent, closing closes the
 * runtime's view window (which the shell syncs back into the strip).
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { editorDrag, setEditorDrag } from "../editor-drag";
import { ChevronDown, GripVertical, X } from "lucide-react";
import { SplitSquareHorizontal, Columns2 } from "lucide-react";
import { useWorkbenchRuntime } from "../runtime-context";
import { cn } from "@/lib/utils";
import { editorKind } from "../editor-registry";
import { useDirtyDocuments } from "../dirty-documents";
import { activateEditor, moveEditor, closeEditor, closeSplit, splitEditor, getGroup, allGroups, mergeGroup, useWorkbenchState, type EditorGroup, type GroupId } from "../workbench-store";

export function EditorTabs(props: { groupId: GroupId; group: EditorGroup }) {
  const dirtyDocuments = useDirtyDocuments();
  const state = useWorkbenchState();
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [overflow, setOverflow] = useState(false);
  const stripRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDetailsElement>(null);
  const dragScroll = useRef<number | null>(null);
  const dragSpeed = useRef(0);
  const stopDragScroll = () => {
    dragSpeed.current = 0;
    if (dragScroll.current !== null) cancelAnimationFrame(dragScroll.current);
    dragScroll.current = null;
  };
  const autoScroll = (clientX: number) => {
    const strip = stripRef.current;
    if (!strip) return;
    const rect = strip.getBoundingClientRect();
    dragSpeed.current = clientX < rect.left + 32 ? -12 : clientX > rect.right - 32 ? 12 : 0;
    if (!dragSpeed.current) { stopDragScroll(); return; }
    if (dragScroll.current !== null) return;
    const tick = () => {
      if (!stripRef.current || !dragSpeed.current) { dragScroll.current = null; return; }
      stripRef.current.scrollLeft += dragSpeed.current;
      dragScroll.current = requestAnimationFrame(tick);
    };
    dragScroll.current = requestAnimationFrame(tick);
  };
  useEffect(() => () => stopDragScroll(), []);
  useEffect(() => {
    const closeMenu = (event: PointerEvent) => {
      if (menuRef.current?.open && !menuRef.current.contains(event.target as Node)) menuRef.current.open = false;
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && menuRef.current?.open) { menuRef.current.open = false; menuRef.current.querySelector<HTMLElement>("summary")?.focus(); }
    };
    document.addEventListener("pointerdown", closeMenu);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", closeMenu); document.removeEventListener("keydown", escape); };
  }, []);
  useEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;
    const update = () => setOverflow(strip.scrollWidth > strip.clientWidth + 1);
    const observer = new ResizeObserver(update);
    observer.observe(strip);
    // Any tab may change width after its title resolves, independently of
    // the viewport or first tab. Track the entire strip's content.
    for (const child of strip.children) observer.observe(child);
    const mutations = new MutationObserver(update);
    mutations.observe(strip, { childList: true, characterData: true, subtree: true });
    const frame = requestAnimationFrame(update);
    update();
    return () => { cancelAnimationFrame(frame); mutations.disconnect(); observer.disconnect(); };
  }, [props.group.editors]);
  useEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;
    const wheel = (event: WheelEvent) => {
      if (strip.scrollWidth <= strip.clientWidth) return;
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (!delta) return;
      event.preventDefault();
      strip.scrollLeft += delta;
    };
    strip.addEventListener("wheel", wheel, { passive: false });
    return () => strip.removeEventListener("wheel", wheel);
  }, []);
  useLayoutEffect(() => {
    const strip = stripRef.current;
    const active = strip?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
    if (!strip || !active) return;
    const viewport = strip.getBoundingClientRect();
    const tab = active.getBoundingClientRect();
    if (tab.left < viewport.left) strip.scrollLeft -= viewport.left - tab.left;
    else if (tab.right > viewport.right) strip.scrollLeft += tab.right - viewport.right;
  }, [props.group.activeId, props.groupId, overflow]);
  const drop = (index: number) => {
    if (!editorDrag) return;
    const drag = editorDrag;
    const dragged = getGroup(state.groups, drag.groupId)?.editors.find((item) => item.id === drag.id);
    if (drag.id) moveEditor(drag.groupId, drag.id, props.groupId, index);
    else mergeGroup(drag.groupId, props.groupId, index);
    if (dragged?.kind === "agent") runtime.selectAgent(String(dragged.params.path ?? ""));
    setEditorDrag(null);
    stopDragScroll();
    setDropIndex(null);
  };
  const runtime = useWorkbenchRuntime();
  const split = allGroups(state.groups).length > 1;
  const isMain = props.groupId === "main";

  return (
    <div
      role="tablist"
      aria-label={isMain ? "编辑器标签" : "分屏编辑器标签"}
      onDragOver={(event) => { if (!editorDrag) return; event.preventDefault(); event.dataTransfer.dropEffect = "move"; setDropIndex(props.group.editors.length); }}
      onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) { stopDragScroll(); setDropIndex(null); } }}
      onDrop={(event) => { if (!editorDrag) return; event.preventDefault(); drop(dropIndex ?? props.group.editors.length); }}
      className="relative z-10 flex h-9 min-w-0 shrink-0 items-stretch border-b bg-muted/40"
    >
      <div ref={stripRef} data-editor-tabs-scroll className="editor-tabs-scroll min-w-0 flex-1 overflow-x-auto overflow-y-hidden" onDragOver={(event) => { if (editorDrag) autoScroll(event.clientX); }} onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) stopDragScroll(); }}>
      <div className="flex h-full w-full min-w-full items-stretch">
      {props.group.editors.map((editor, index) => {
        const kind = editorKind(editor.kind);
        if (!kind) return null;
        const active = editor.id === props.group.activeId;
        const Icon = kind.icon?.(editor);
        const title = kind.title(editor);
        const longName = kind.name?.(editor) ?? title;
        const isAgent = editor.kind === "agent";
        const resource = kind.resource?.(editor);
        const dirty = resource ? dirtyDocuments.includes(resource) : Boolean((editor.state as { dirty?: boolean } | null)?.dirty);
        return (
          // VSCode-shaped tab: flat cells on the strip; the active tab
          // lifts to the background color with a 2px accent on top. The
          // title attribute carries the long name (agent = full path).
          <div
            key={editor.id}
            role="tab"
            data-dirty={dirty || undefined}
            draggable
            onDragStart={(event) => { setEditorDrag({ groupId: props.groupId, id: editor.id }); event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", editor.id); }}
            onDragEnd={() => { setEditorDrag(null); stopDragScroll(); setDropIndex(null); }}
            onDragOver={(event) => { if (!editorDrag) return; event.preventDefault(); event.stopPropagation(); autoScroll(event.clientX); event.dataTransfer.dropEffect = "move"; const rect = event.currentTarget.getBoundingClientRect(); setDropIndex(index + (event.clientX >= rect.left + rect.width / 2 ? 1 : 0)); }}
            onDrop={(event) => { if (!editorDrag) return; event.preventDefault(); event.stopPropagation(); drop(dropIndex ?? index); }}
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            onKeyDown={(event) => {
              if (event.target !== event.currentTarget || !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
              event.preventDefault();
              const editors = props.group.editors;
              const next = event.key === "Home" ? 0 : event.key === "End" ? editors.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + editors.length) % editors.length;
              const target = editors[next];
              if (!target) return;
              activateEditor(props.groupId, target.id);
              if (target.kind === "agent") runtime.selectAgent(String(target.params.path ?? ""));
              requestAnimationFrame(() => stripRef.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus({ preventScroll: true }));
            }}
            aria-label={title}
            title={longName}
            onClick={() => {
              activateEditor(props.groupId, editor.id);
              if (isAgent) runtime.selectAgent(String(editor.params.path ?? ""));
            }}
            className={cn(
              "group/tab relative flex min-w-20 max-w-48 shrink cursor-pointer items-center gap-1.5 border-r px-2.5 text-xs",
              dropIndex === index && "before:absolute before:inset-y-0 before:left-0 before:z-10 before:w-0.5 before:bg-primary",
              dropIndex === props.group.editors.length && index === props.group.editors.length - 1 && "after:absolute after:inset-y-0 after:right-0 after:z-10 after:w-0.5 after:bg-primary",
              active
                ? "bg-background text-foreground shadow-[inset_0_2px_0_0] shadow-primary"
                : "text-muted-foreground hover:bg-accent/60",
            )}
          >
            {Icon && <Icon className="size-3.5 shrink-0" aria-hidden />}
            <span className="min-w-0 truncate">{title}</span>
            {dirty && <span aria-hidden="true" title="未保存修改" className="size-1.5 shrink-0 rounded-full bg-current" />}
            <button
              type="button"
              aria-label={`关闭 ${title}`}
              title={`关闭 ${title}`}
              disabled={editor.pinned}
              onClick={(event) => {
                event.stopPropagation();
                if (isAgent) runtime.closeTab(String(editor.params.path ?? ""));
                else closeEditor(editor.id);
              }}
              className="rounded-sm p-0.5 opacity-0 transition hover:bg-accent focus-visible:opacity-100 group-hover/tab:opacity-100 disabled:opacity-30"
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
      </div>
      </div>
      <div className="flex shrink-0 items-center gap-0.5 px-1.5">
        {split && <button type="button" draggable aria-label="拖动编辑器组" title="拖动编辑器组" className="grid size-6 cursor-grab place-items-center rounded-sm text-muted-foreground hover:bg-accent" onDragStart={event => { setEditorDrag({ groupId: props.groupId, id: "" }); event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", "editor-group"); }} onDragEnd={() => { setEditorDrag(null); stopDragScroll(); setDropIndex(null); }}><GripVertical className="size-3.5" /></button>}
        {overflow && <details ref={menuRef} className="relative">
          <summary aria-label="显示所有编辑器标签" title="显示所有编辑器标签" className="grid size-6 cursor-pointer list-none place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground [&::-webkit-details-marker]:hidden"><ChevronDown className="size-4" /></summary>
          <div className="absolute right-0 top-full z-50 max-h-80 min-w-48 max-w-72 overflow-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-lg" role="menu" aria-label="所有编辑器标签">
            {props.group.editors.map((editor) => {
              const kind = editorKind(editor.kind);
              if (!kind) return null;
              const Icon = kind.icon?.(editor);
              return <button key={editor.id} type="button" role="menuitem" title={kind.name?.(editor) ?? kind.title(editor)} className={cn("flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs hover:bg-accent", editor.id === props.group.activeId && "bg-accent")} onClick={() => {
                activateEditor(props.groupId, editor.id);
                if (editor.kind === "agent") runtime.selectAgent(String(editor.params.path ?? ""));
                if (menuRef.current) menuRef.current.open = false;
              }}>{Icon && <Icon className="size-3.5 shrink-0" />}<span className="truncate">{kind.title(editor)}</span></button>;
            })}
          </div>
        </details>}
        {state.groups.active === props.groupId && (
          <button
            type="button"
            aria-label="拆分编辑器"
            title="拆分编辑器"
            disabled={!props.group.activeId || props.group.editors.length < 2}
            onClick={() => splitEditor(props.groupId)}
            className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
          >
            <SplitSquareHorizontal className="size-3.5" />
          </button>
        )}
        {split && (
          <button
            type="button"
            aria-label={props.groupId === "secondary" ? "关闭分屏" : "关闭编辑器组"}
            title={props.groupId === "secondary" ? "关闭分屏" : "关闭编辑器组"}
            onClick={() => closeSplit(props.groupId)}
            className="grid size-6 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <Columns2 className="size-3.5" />
          </button>
        )}
      </div>
    </div>
  );
}
