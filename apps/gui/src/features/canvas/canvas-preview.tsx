import { useEffect, useRef } from "react";
import { Excalidraw } from "@excalidraw/excalidraw";
import type { ExcalidrawInitialDataState, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
export type PreviewFocus = { id: string; request: number } | null;
export function CanvasPreview({ document, focus = null }: { document: ExcalidrawInitialDataState; focus?: PreviewFocus }) {
  const api = useRef<ExcalidrawImperativeAPI | null>(null);
  const container = useRef<HTMLDivElement | null>(null);
  const fitted = useRef(false);
  const focusRef = useRef(focus);
  focusRef.current = focus;
  const missing = focus && !(document.elements ?? []).some(element => element.id === focus.id && !element.isDeleted);
  function fit() {
    const elements = api.current?.getSceneElements();
    if (!elements?.length) return;
    const target = focusRef.current && elements.find(element => element.id === focusRef.current!.id);
    api.current?.scrollToContent(target ? [target] : elements, { fitToViewport: true, maxZoom: target ? 1.5 : 1 });
  }
  useEffect(() => { fit(); }, [focus]);
  useEffect(() => {
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(fit);
    });
    if (container.current) observer.observe(container.current);
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, []);
  return <div ref={container} className="relative h-full w-full" data-preview-focus={focus?.id ?? ""}>
    {focus && <p className="pointer-events-none absolute left-2 top-2 z-10 max-w-[90%] break-all rounded border bg-background/95 px-2 py-1 text-xs" role="status">{missing ? `此版本无对象：${focus.id}` : `定位对象：${focus.id}`}</p>}
    <Excalidraw initialData={document} viewModeEnabled langCode="zh-CN" handleKeyboardGlobally={false} excalidrawAPI={instance => { api.current = instance; }} onChange={elements => {
      if (elements.length && !fitted.current && api.current) { fitted.current = true; fit(); }
    }} />
  </div>;
}
