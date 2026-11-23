import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Sidebar, CaptureUpdateAction, newElementWith } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { ArrowDown, ArrowDownToLine, ArrowUp, ArrowUpToLine, ChevronDown, ChevronRight, Eye, EyeOff, Lock, Pencil, Unlock } from "lucide-react";
import { canvasElementName, canvasElementType, canvasLayers, isLayerHidden, layerLock, layerMetadata, layerVisibility, type CanvasLayer } from "./canvas-layers-model";
import "./canvas-layers.css";

export function CanvasLayers({ getApi }: { getApi: () => ExcalidrawImperativeAPI | null }) {
  const { t } = useTranslation("canvas");
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
    <Sidebar.Header><span className="canvas-layers-title">{t("layers.title")}</span></Sidebar.Header>
    <section ref={panel} aria-label={t("layers.panelLabel")} className="canvas-layers-body">
      <div role="toolbar" aria-label={t("layers.orderLabel")} className="canvas-layers-order">
        {([["top", t("layers.moveToTop"), ArrowUpToLine], ["up", t("layers.moveUp"), ArrowUp], ["down", t("layers.moveDown"), ArrowDown], ["bottom", t("layers.moveToBottom"), ArrowDownToLine]] as const).map(([direction, label, Icon]) => <button key={direction} type="button" title={label} aria-label={label} disabled={!active || (direction === "up" || direction === "top" ? active.id === rows[0]?.id : active.id === rows.at(-1)?.id)} onClick={() => reorder(direction)}><Icon size={16} /></button>)}
        <span>{t("layers.layerCount", { count: rows.length })}</span>
      </div>
      <p className="canvas-layers-hint">{t("layers.hint")}</p>
      <div aria-label={t("layers.listLabel")} className="canvas-layers-list">
        {!rows.length && <p className="canvas-layers-hint">{t("layers.empty")}</p>}
        {rows.map(row => <div key={row.id} className="canvas-layer-item"><div data-layer-id={row.id} className="canvas-layer-row" data-active={active?.id === row.id}>
          {row.ids.length > 1 ? <button type="button" className="canvas-layer-expand" aria-label={expanded[row.id] ? t("layers.collapseLabel") : t("layers.expandLabel")} aria-expanded={Boolean(expanded[row.id])} aria-controls={`${treeId}-${row.id}`} onClick={() => setExpanded(current => ({ ...current, [row.id]: !current[row.id] }))}>{expanded[row.id] ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</button> : <span className="canvas-layer-expand-spacer" />}
          <button type="button" className="canvas-layer-name" title={row.name} aria-pressed={row.ids.some(id => snapshot.selected[id])} disabled={row.hidden || row.locked} onClick={() => select(row)}>{row.name}</button>
          {row.ids.length > 1 && <span className="canvas-layer-count" title={t("layers.elementCount", { count: row.ids.length })}>{row.ids.length}</span>}
          <button type="button" title={t("layers.renameLayer")} aria-label={t("layers.renameLayer")} onClick={() => {
            const name = window.prompt(t("layers.renameLayerPrompt"), row.name)?.trim(); if (!name) return;
            change(row, element => element.id === row.id ? { customData: { ...element.customData, canvasLayer: { ...layerMetadata(element), name: name.slice(0, 200) } } } : {});
          }}><Pencil size={14} /></button>
          <button type="button" title={row.hidden ? t("layers.showLayer") : t("layers.hideLayer")} aria-label={row.hidden ? t("layers.showLayer") : t("layers.hideLayer")} aria-pressed={row.hidden} onClick={() => change(row, element => layerVisibility(element, !row.hidden), !row.hidden)}>{row.hidden ? <EyeOff size={16} /> : <Eye size={16} />}</button>
          <button type="button" title={row.locked ? t("layers.unlockLayer") : t("layers.lockLayer")} aria-label={row.locked ? t("layers.unlockLayer") : t("layers.lockLayer")} aria-pressed={row.locked} onClick={() => change(row, element => layerLock(element, !row.locked), !row.locked)}>{row.locked ? <Lock size={14} /> : <Unlock size={14} />}</button>
        </div>
        {row.ids.length > 1 && expanded[row.id] && <div id={`${treeId}-${row.id}`} role="group" aria-label={t("layers.elementsIn", { name: row.name })} className="canvas-layer-children">
          {[...row.ids].reverse().map(id => {
            const element = elementsById.get(id); if (!element) return null;
            const name = canvasElementName(element);
            return <div key={id} data-layer-element-id={id} className="canvas-layer-element-row"><button type="button" className="canvas-layer-element" title={`${canvasElementType(element)} · ${name}`} aria-pressed={Boolean(snapshot.selected[id])} disabled={isLayerHidden(element) || element.locked} onClick={() => select(row, [id], { individual: true })}>
              <span className="canvas-layer-element-type">{canvasElementType(element)}</span><span className="canvas-layer-element-name">{name}</span>
            </button><button type="button" className="canvas-layer-element-rename" title={t("layers.renameElement")} aria-label={t("layers.renameElement")} onClick={() => {
              const renamed = window.prompt(t("layers.renameElementPrompt"), name)?.trim();
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
