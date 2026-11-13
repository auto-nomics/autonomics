import { useEffect, useRef, useState } from "react";
import { getCommonBounds, viewportCoordsToSceneCoords } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
type Point = { x: number; y: number };
/** Temporary review selection overlay. It never inserts publication artwork. */
export function CanvasRectangleSelect({ getApi, onDone }: { getApi: () => ExcalidrawImperativeAPI | null; onDone: (count: number, error?: string) => void }) {
  const layer = useRef<SVGSVGElement>(null);
  const anchor = useRef<Point | null>(null);
  const [region, setRegion] = useState<{ start: Point; end: Point } | null>(null);
  useEffect(() => { layer.current?.focus(); }, []);
  const point = (event: React.PointerEvent<SVGSVGElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  };
  return <svg ref={layer} role="application" aria-label="框选批注区域" tabIndex={0} className="absolute inset-0 z-20 h-full w-full touch-none cursor-crosshair outline-none" onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onDone(0); } }}
    onPointerDown={event => {
      if (event.button !== 0) return; event.preventDefault(); event.stopPropagation();
      event.currentTarget.setPointerCapture(event.pointerId); anchor.current = point(event); setRegion({ start: anchor.current, end: anchor.current });
    }} onPointerMove={event => { if (anchor.current) { event.preventDefault(); setRegion({ start: anchor.current, end: point(event) }); } }}
    onPointerCancel={() => { anchor.current = null; onDone(0); }} onPointerUp={event => {
      const start = anchor.current; anchor.current = null; if (!start) return;
      event.preventDefault(); event.stopPropagation();
      const end = point(event); const api = getApi(); if (!api) { onDone(0); return; }
      if (Math.abs(end.x - start.x) < 8 || Math.abs(end.y - start.y) < 8) { onDone(0, "请拖动框选需要批注的对象。"); return; }
      const bounds = event.currentTarget.getBoundingClientRect(); const state = api.getAppState();
      const a = viewportCoordsToSceneCoords({ clientX: bounds.left + start.x, clientY: bounds.top + start.y }, state);
      const b = viewportCoordsToSceneCoords({ clientX: bounds.left + end.x, clientY: bounds.top + end.y }, state);
      const minX = Math.min(a.x, b.x), minY = Math.min(a.y, b.y);
      const maxX = Math.max(a.x, b.x), maxY = Math.max(a.y, b.y);
      const selected = api.getSceneElements().filter(element => {
        const [left, top, right, bottom] = getCommonBounds([element]);
        return right >= minX && left <= maxX && bottom >= minY && top <= maxY;
      });
      if (selected.length > 100) { onDone(0, "最多框选 100 个对象，请缩小范围。"); return; }
      api.updateScene({ appState: { selectedElementIds: Object.fromEntries(selected.map(element => [element.id, true])) } });
      onDone(selected.length, selected.length ? undefined : "框选区域中没有对象。");
    }}>
    <rect width="100%" height="100%" fill="transparent" />
    {region && <rect x={Math.min(region.start.x, region.end.x)} y={Math.min(region.start.y, region.end.y)} width={Math.abs(region.end.x - region.start.x)} height={Math.abs(region.end.y - region.start.y)} fill="#ef444414" stroke="#ef4444" strokeWidth="2" strokeDasharray="6 3" />}
    <text x="16" y="32" fill="#b91c1c" fontSize="14">拖动框选需要批注的对象 · Esc 取消</text>
  </svg>;
}
