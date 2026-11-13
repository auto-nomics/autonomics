import { useEffect, useId, useRef, useState } from "react";
import { Sidebar, CaptureUpdateAction, newElementWith } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { ArrowDown, ArrowDownToLine, ArrowUp, ArrowUpToLine, ChevronDown, ChevronRight, Eye, EyeOff, Lock, Pencil, Unlock } from "lucide-react";
import { canvasElementName, canvasElementType, canvasLayers, isLayerHidden, layerLock, layerMetadata, layerVisibility, type CanvasLayer } from "./canvas-layers-model";
import "./canvas-layers.css";

export function CanvasLayers({ getApi }: { getApi: () => ExcalidrawImperativeAPI | null }) {
  const [open, setOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<{ elements: readonly ExcalidrawElement[]; selected: Record<string, boolean> }>({ elements: [], selected: {} });
  const [activeId, setActiveId] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const treeId = useId();
  const panel = useRef<HTMLElement>(null);
  useEffect(() => {
    const api = getApi(); if (!api || !open) return;
    const refresh = () => setSnapshot({ elements: [...api.getSceneElements()], selected: { ...api.getAppState().selectedElementIds } });
    refresh(); return api.onChange(refresh);
  }, [getApi, open]);
  const rows = canvasLayers(snapshot.elements);
  const elementsById = new Map(snapshot.elements.map(element => [element.id, element]));
  const active = rows.find(row => row.ids.some(id => snapshot.selected[id])) ?? rows.find(row => row.id === activeId);
  const select = (row: CanvasLayer, ids = row.ids, options: { individual?: boolean; reveal?: boolean } = {}) => {
    const api = getApi(); if (!api) return;
    const elements = api.getSceneElements().filter(element => ids.includes(element.id));
    const commonGroups = elements[0]?.groupIds.filter(group => elements.every(element => element.groupIds.includes(group))) ?? [];
    const group = commonGroups.at(-1);
    setActiveId(row.id); api.setActiveTool({ type: "selection" });
    api.updateScene({ appState: {
      selectedElementIds: Object.fromEntries(ids.map(id => [id, true])),
      selectedGroupIds: !options.individual && group ? { [group]: true } : {},
      editingGroupId: options.individual ? elements[0]?.groupIds[0] ?? null : null,
    } });
    if (options.reveal !== false) api.scrollToContent(elements, { fitToContent: false });
  };
  const change = (row: CanvasLayer, patch: (element: ExcalidrawElement) => Partial<Pick<ExcalidrawElement, "opacity" | "locked" | "customData">>, clearSelection = false) => {
    const api = getApi(); if (!api) return;
    const ids = new Set(row.ids);
    api.updateScene({ elements: api.getSceneElementsIncludingDeleted().map(element => ids.has(element.id) ? newElementWith(element, patch(element)) : element),
      ...(clearSelection ? { appState: { selectedElementIds: Object.fromEntries(Object.entries(api.getAppState().selectedElementIds).filter(([id]) => !ids.has(id))), selectedGroupIds: {}, editingGroupId: null } } : {}),
      captureUpdate: CaptureUpdateAction.IMMEDIATELY });
    setActiveId(row.id);
  };
  const reorder = (direction: "up" | "down" | "top" | "bottom") => {
    const api = getApi(), canvas = panel.current?.closest<HTMLElement>(".excalidraw");
    if (!api || !canvas || !active) return;
    const ids = [...active.ids];
    select(active, ids, { reveal: false });
    // Use the native action pipeline so frame ownership and undo snapshots match
    // the canvas's own layer shortcuts, including pending image initialization.
    requestAnimationFrame(() => {
      const selected = api.getAppState().selectedElementIds;
      if (ids.some(id => !selected[id]) || Object.keys(selected).filter(id => selected[id]).length !== ids.length) return;
      const forward = direction === "up" || direction === "top";
      const mac = navigator.platform.includes("Mac");
      canvas.focus({ preventScroll: true });
      canvas.dispatchEvent(new KeyboardEvent("keydown", { key: forward ? "]" : "[", code: forward ? "BracketRight" : "BracketLeft", ctrlKey: !mac, metaKey: mac, shiftKey: direction === "top" || direction === "bottom", bubbles: true, cancelable: true }));
    });
  };
  return <Sidebar name="layers" className="canvas-layers" onStateChange={state => setOpen(Boolean(state))}>
    <Sidebar.Header><span className="canvas-layers-title">图层</span></Sidebar.Header>
    <section ref={panel} aria-label="图层面板" className="canvas-layers-body">
      <div role="toolbar" aria-label="图层顺序" className="canvas-layers-order">
        {([["top", "置顶", ArrowUpToLine], ["up", "上移一层", ArrowUp], ["down", "下移一层", ArrowDown], ["bottom", "置底", ArrowDownToLine]] as const).map(([direction, label, Icon]) => <button key={direction} type="button" title={label} aria-label={label} disabled={!active || (direction === "up" || direction === "top" ? active.id === rows[0]?.id : active.id === rows.at(-1)?.id)} onClick={() => reorder(direction)}><Icon size={16} /></button>)}
        <span>{rows.length} 个图层</span>
      </div>
      <p className="canvas-layers-hint">上方图层显示在前，已有分组和画框整体操作。</p>
      <div aria-label="图层列表" className="canvas-layers-list">
        {!rows.length && <p className="canvas-layers-hint">画布上添加对象后会显示在这里。</p>}
        {rows.map(row => <div key={row.id} className="canvas-layer-item"><div data-layer-id={row.id} className="canvas-layer-row" data-active={active?.id === row.id}>
          {row.ids.length > 1 ? <button type="button" className="canvas-layer-expand" aria-label={expanded[row.id] ? "收起图层元素" : "展开图层元素"} aria-expanded={Boolean(expanded[row.id])} aria-controls={`${treeId}-${row.id}`} onClick={() => setExpanded(current => ({ ...current, [row.id]: !current[row.id] }))}>{expanded[row.id] ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</button> : <span className="canvas-layer-expand-spacer" />}
          <button type="button" className="canvas-layer-name" title={row.name} aria-pressed={row.ids.some(id => snapshot.selected[id])} disabled={row.hidden || row.locked} onClick={() => select(row)}>{row.name}</button>
          {row.ids.length > 1 && <span className="canvas-layer-count" title={`${row.ids.length} 个元素`}>{row.ids.length}</span>}
          <button type="button" title="重命名图层" aria-label="重命名图层" onClick={() => {
            const name = window.prompt("重命名图层", row.name)?.trim(); if (!name) return;
            change(row, element => element.id === row.id ? { customData: { ...element.customData, canvasLayer: { ...layerMetadata(element), name: name.slice(0, 200) } } } : {});
          }}><Pencil size={14} /></button>
          <button type="button" title={row.hidden ? "显示图层" : "隐藏图层"} aria-label={row.hidden ? "显示图层" : "隐藏图层"} aria-pressed={row.hidden} onClick={() => change(row, element => layerVisibility(element, !row.hidden), !row.hidden)}>{row.hidden ? <EyeOff size={16} /> : <Eye size={16} />}</button>
          <button type="button" title={row.locked ? "解锁图层" : "锁定图层"} aria-label={row.locked ? "解锁图层" : "锁定图层"} aria-pressed={row.locked} onClick={() => change(row, element => layerLock(element, !row.locked), !row.locked)}>{row.locked ? <Lock size={14} /> : <Unlock size={14} />}</button>
        </div>
        {row.ids.length > 1 && expanded[row.id] && <div id={`${treeId}-${row.id}`} role="group" aria-label={`${row.name}内的元素`} className="canvas-layer-children">
          {[...row.ids].reverse().map(id => {
            const element = elementsById.get(id); if (!element) return null;
            const name = canvasElementName(element);
            return <div key={id} data-layer-element-id={id} className="canvas-layer-element-row"><button type="button" className="canvas-layer-element" title={`${canvasElementType(element)} · ${name}`} aria-pressed={Boolean(snapshot.selected[id])} disabled={isLayerHidden(element) || element.locked} onClick={() => select(row, [id], { individual: true })}>
              <span className="canvas-layer-element-type">{canvasElementType(element)}</span><span className="canvas-layer-element-name">{name}</span>
            </button><button type="button" className="canvas-layer-element-rename" title="重命名元素" aria-label="重命名元素" onClick={() => {
              const renamed = window.prompt("重命名元素", name)?.trim();
              if (!renamed || renamed === name) return;
              change(row, member => member.id === id ? { customData: { ...member.customData, canvasLayer: { ...layerMetadata(member), elementName: renamed.slice(0, 200) } } } : {});
            }}><Pencil size={14} /></button></div>;
          })}
        </div>}
        </div>)}
      </div>
    </section>
  </Sidebar>;
}
