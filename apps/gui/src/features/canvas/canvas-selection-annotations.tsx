import { useEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Popover } from "@base-ui/react/popover";
import { getCommonBounds, sceneCoordsToViewportCoords } from "@excalidraw/excalidraw";
import { MessageSquarePlus, X } from "lucide-react";
import { CanvasAnnotations } from "./canvas-annotations";
import { useCanvasAgentHandoff, type CanvasAgentHandoffProps } from "./use-canvas-agent-handoff";
import "./canvas-selection-annotations.css";

type SelectionAnchor = { ids: string[]; key: string; x: number; y: number; visible: boolean; theme: CSSProperties };
export function CanvasSelectionAnnotations({ surface, agentPath, ...props }: CanvasAgentHandoffProps & { surface: RefObject<HTMLDivElement | null>; agentPath: string }) {
  const { t } = useTranslation("canvas");
  const [selection, setSelection] = useState<SelectionAnchor | null>(null);
  const [open, setOpen] = useState(false);
  const [targets, setTargets] = useState<string[]>([]);
  const selectionKey = useRef("");
  const popup = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const handoff = useCanvasAgentHandoff({ ...props, target: agentPath });
  useEffect(() => {
    const api = props.getApi(), container = surface.current; if (!api || !container) return;
    const refresh = () => {
      const state = api.getAppState();
      const elements = api.getSceneElements().filter(element => state.selectedElementIds[element.id]);
      const ids = elements.map(element => element.id).sort(), key = JSON.stringify(ids);
      if (key !== selectionKey.current) { selectionKey.current = key; setOpen(false); }
      if (!elements.length) { setSelection(null); return; }
      const [left, top, right, bottom] = getCommonBounds(elements);
      const start = sceneCoordsToViewportCoords({ sceneX: left, sceneY: top }, state);
      const end = sceneCoordsToViewportCoords({ sceneX: right, sceneY: bottom }, state);
      const bounds = container.getBoundingClientRect();
      const native = container.querySelector('.excalidraw');
      const colors = native ? getComputedStyle(native) : null;
      const theme = {
        "--annotation-background": colors?.getPropertyValue('--island-bg-color').trim() || '#fff',
        "--annotation-foreground": colors?.getPropertyValue('--color-on-surface').trim() || '#1b1b1f',
        "--annotation-border": colors?.getPropertyValue('--default-border-color').trim() || '#ddd',
      } as CSSProperties;
      const visible = state.cursorButton === 'up' && state.activeTool.type === 'selection' && !state.selectionElement && !state.isResizing && !state.isRotating && !state.editingTextElement
        && end.x >= bounds.left && start.x <= bounds.right && end.y >= bounds.top && start.y <= bounds.bottom;
      if (!visible) setOpen(false);
      const next = { ids, key, visible, x: Math.max(8, Math.min(bounds.width - 108, end.x - bounds.left + 10)), y: Math.max(56, Math.min(bounds.height - 84, start.y - bounds.top - 42)), theme };
      setSelection(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    };
    refresh(); const unsubscribe = api.onChange(refresh);
    const observer = new ResizeObserver(refresh); observer.observe(container);
    return () => { unsubscribe(); observer.disconnect(); };
  }, [props.getApi, surface]);
  return <Popover.Root open={open && Boolean(selection)} onOpenChange={next => {
    if (next && selection) setTargets(selection.ids);
    setOpen(next);
  }} modal={false}>
    {selection?.visible && <Popover.Trigger ref={trigger} className="canvas-annotation-trigger" aria-label={t("selectionAnnotations.triggerLabel")} title={selection.ids.length > 100 ? t("selectionAnnotations.tooManyObjects") : t("selectionAnnotations.addTarget", { count: selection.ids.length })} disabled={selection.ids.length > 100} data-prevent-outside-click="true" style={{ ...selection.theme, left: selection.x, top: selection.y }} onPointerDown={event => event.stopPropagation()}><MessageSquarePlus size={16} /><span>{t("selectionAnnotations.annotate")}</span><span className="canvas-annotation-count" aria-hidden="true">{selection.ids.length}</span></Popover.Trigger>}
    <Popover.Portal container={surface} keepMounted>
      <Popover.Positioner side="right" align="start" sideOffset={8} collisionPadding={8} collisionBoundary={surface.current ?? undefined} className="canvas-annotation-positioner">
        <Popover.Popup ref={popup} role="dialog" className="canvas-annotation-popover" style={selection?.theme} data-prevent-outside-click="true" initialFocus={() => popup.current?.querySelector<HTMLTextAreaElement>("textarea") ?? true} finalFocus={false} onPointerDown={event => event.stopPropagation()} onKeyDown={event => { if (event.key === "Escape") { setOpen(false); trigger.current?.focus(); } event.stopPropagation(); }}>
          <div className="canvas-annotation-header"><Popover.Title>{t("annotations.title")}</Popover.Title><span className="canvas-annotation-target-count">{t("selectionAnnotations.objectCount", { count: targets.length })}</span><Popover.Close aria-label={t("selectionAnnotations.closeLabel")} disabled={handoff.busy}><X size={16} /></Popover.Close></div>
          <CanvasAnnotations key={JSON.stringify(targets)} {...props} elementIds={targets} active={open} preparing={handoff.busy} canPrepare={handoff.targetAvailable} onPrepare={async annotation => {
            const prepared = await handoff.prepare(annotation.note, annotation);
            if (prepared) setOpen(false);
            return prepared;
          }} />
          {handoff.error && <p role="alert" className="text-destructive">{handoff.error}</p>}
        </Popover.Popup>
      </Popover.Positioner>
    </Popover.Portal>
  </Popover.Root>;
}
