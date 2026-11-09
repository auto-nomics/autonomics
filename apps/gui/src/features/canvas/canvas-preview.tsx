import { useRef } from "react";
import { Excalidraw } from "@excalidraw/excalidraw";
import type { ExcalidrawInitialDataState, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
export function CanvasPreview({ document }: { document: ExcalidrawInitialDataState }) {
  const api = useRef<ExcalidrawImperativeAPI | null>(null);
  const fitted = useRef(false);
  return <Excalidraw initialData={document} viewModeEnabled langCode="zh-CN" handleKeyboardGlobally={false} excalidrawAPI={instance => { api.current = instance; }} onChange={elements => {
    if (elements.length && !fitted.current && api.current) { fitted.current = true; api.current.scrollToContent(elements, { fitToViewport: true, maxZoom: 1 }); }
  }} />;
}
