import { useEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { DagSnapshot } from "@/types";
import { calculateLevels } from "./dag";

type Point = { x: number; y: number };
type View = { zoom: number; x: number; y: number };

/** Compact overview of the active agent's DAG. */
export function DagMinimap({ activeAgent }: { activeAgent: string }) {
  const [snapshot, setSnapshot] = useState<DagSnapshot | null>(null);
  const [error, setError] = useState(false);
  const [view, setView] = useState<View>({ zoom: 1, x: 0, y: 0 });
  const [svg, setSvg] = useState<SVGSVGElement | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;

  useEffect(() => {
    const controller = new AbortController();
    let inFlight = false;
    const load = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const data = await gatewayRequest("GET", `/agents/${activeAgent}/dag`, undefined, { signal: controller.signal }) as DagSnapshot;
        if (!controller.signal.aborted) { setSnapshot(data); setError(false); }
      } catch {
        if (!controller.signal.aborted) setError(true);
      } finally { inFlight = false; }
    };
    void load();
    const timer = window.setInterval(load, 5000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [activeAgent]);

  const layout = useMemo(() => {
    if (!snapshot?.nodes.length) return null;
    const levels = calculateLevels(snapshot);
    const columns = new Map<number, string[]>();
    for (const node of snapshot.nodes) {
      const level = levels.get(node.id) ?? 0;
      columns.set(level, [...(columns.get(level) ?? []), node.id]);
    }
    const positions = new Map<string, Point>();
    const rows = Math.max(...Array.from(columns.values(), items => items.length));
    for (const [level, ids] of columns) {
      ids.forEach((id, row) => positions.set(id, { x: 25 + level * 65, y: 20 + row * 35 + (rows - ids.length) * 17.5 }));
    }
    const contentWidth = Math.max(50, (Math.max(...columns.keys()) + 1) * 65 - 15);
    const contentHeight = Math.max(40, rows * 35 + 5);
    const width = Math.max(contentWidth, contentHeight * 300 / 140);
    const height = Math.max(contentHeight, width * 140 / 300);
    return { positions, width, height };
  }, [snapshot]);

  useEffect(() => {
    if (!svg || !layout) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const current = viewRef.current;
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? svg.clientHeight : 1);
      const zoom = Math.max(1, Math.min(8, current.zoom * Math.exp(-delta * 0.002)));
      if (zoom === current.zoom) return;
      const matrix = svg.getScreenCTM();
      if (!matrix) return;
      const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
      const ratio = current.zoom / zoom;
      const next = zoom === 1
        ? { zoom, x: 0, y: 0 }
        : { zoom, x: point.x - (point.x - current.x) * ratio, y: point.y - (point.y - current.y) * ratio };
      viewRef.current = next;
      setView(next);
    };
    svg.addEventListener("wheel", onWheel, { passive: false });
    return () => svg.removeEventListener("wheel", onWheel);
  }, [svg, layout]);

  if (!snapshot) return <div className="grid h-32 place-items-center rounded-md border text-xs text-muted-foreground">{error ? "Could not load DAG" : "Loading DAG…"}</div>;
  if (!layout) return <div className="grid h-32 place-items-center rounded-md border text-xs text-muted-foreground">{error ? "Could not refresh DAG" : "No DAG nodes"}</div>;

  return (
    <div className="relative overflow-hidden rounded-md border bg-card">
      <svg ref={setSvg} role="img" aria-label={`DAG overview: ${snapshot.nodes.length} nodes, ${snapshot.edges.length} edges. Scroll to zoom.`} className="block h-32 w-full" viewBox={`${view.x} ${view.y} ${layout.width / view.zoom} ${layout.height / view.zoom}`}>
        {snapshot.edges.map((edge, index) => {
          const from = layout.positions.get(edge.from), to = layout.positions.get(edge.to);
          return from && to ? <line key={`${edge.from}:${edge.to}:${index}`} x1={from.x} y1={from.y} x2={to.x} y2={to.y} stroke="currentColor" className="text-muted-foreground/50" strokeWidth="1.5" /> : null;
        })}
        {snapshot.nodes.map(node => {
          const point = layout.positions.get(node.id);
          return point && <circle key={node.id} cx={point.x} cy={point.y} r="4" fill="currentColor" className="text-primary"><title>{node.id}</title></circle>;
        })}
      </svg>
      {error && <span className="absolute right-1 bottom-1 text-xs text-destructive">Refresh failed</span>}
    </div>
  );
}
