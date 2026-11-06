import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { DagSnapshot } from "@/types";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { RefreshCw, ZoomIn, ZoomOut, GitBranch } from "lucide-react";

export function dagStatus(status: unknown) {
  if (typeof status === "string") return status.toLowerCase();
  if (status && typeof status === "object") return Object.keys(status)[0]?.toLowerCase() ?? "pending";
  return "pending";
}

const statusColors: Record<string, { node: string; badge: string }> = {
  pending: { node: "border-l-slate-400 bg-slate-500/5", badge: "bg-slate-500/10 text-slate-600 dark:text-slate-300" },
  ready: { node: "border-l-sky-500 bg-sky-500/5", badge: "bg-sky-500/10 text-sky-700 dark:text-sky-300" },
  running: { node: "border-l-amber-500 bg-amber-500/5", badge: "bg-amber-500/10 text-amber-700 dark:text-amber-300" },
  success: { node: "border-l-emerald-500 bg-emerald-500/5", badge: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" },
  failed: { node: "border-l-red-500 bg-red-500/5", badge: "bg-red-500/10 text-red-700 dark:text-red-300" },
  skipped: { node: "border-l-violet-500 bg-violet-500/5", badge: "bg-violet-500/10 text-violet-700 dark:text-violet-300" },
  cancelled: { node: "border-l-orange-500 bg-orange-500/5", badge: "bg-orange-500/10 text-orange-700 dark:text-orange-300" },
};

export function calculateLevels(snapshot: DagSnapshot) {
  const incoming = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const outgoing = new Map(snapshot.nodes.map((node) => [node.id, [] as string[]]));
  for (const edge of snapshot.edges) {
    incoming.set(edge.to, (incoming.get(edge.to) ?? 0) + 1);
    outgoing.get(edge.from)?.push(edge.to);
  }
  const levels = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const remaining = new Map(incoming);
  const queue = snapshot.nodes.filter((node) => (incoming.get(node.id) ?? 0) === 0).map((node) => node.id);
  for (let cursor = 0; cursor < queue.length; cursor++) {
    const id = queue[cursor];
    for (const next of outgoing.get(id) ?? []) {
      levels.set(next, Math.max(levels.get(next) ?? 0, (levels.get(id) ?? 0) + 1));
      const count = (remaining.get(next) ?? 1) - 1;
      remaining.set(next, count);
      if (count === 0) queue.push(next);
    }
  }
  return levels;
}

export function DagView({ activeAgent }: { activeAgent: string }) {
  const [snapshot, setSnapshot] = useState<DagSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [zoom, setZoom] = useState(1);
  const zoomRef = useRef(1);
  const zoomAnchor = useRef<{ x: number; y: number; graphX: number; graphY: number } | null>(null);

  useEffect(() => {
    if (!viewport) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const oldZoom = zoomRef.current;
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
      const nextZoom = Math.min(3, Math.max(0.25, oldZoom * Math.exp(-delta * 0.002)));
      if (nextZoom === oldZoom) return;
      const rect = viewport.getBoundingClientRect();
      const x = event.clientX - rect.left;
      const y = event.clientY - rect.top;
      const previous = zoomAnchor.current;
      // Multiple wheel events can arrive before React commits the new size.
      const scrollX = previous ? previous.graphX * oldZoom - previous.x : viewport.scrollLeft;
      const scrollY = previous ? previous.graphY * oldZoom - previous.y : viewport.scrollTop;
      zoomAnchor.current = { x, y, graphX: (scrollX + x) / oldZoom, graphY: (scrollY + y) / oldZoom };
      zoomRef.current = nextZoom;
      setZoom(nextZoom);
    };
    viewport.addEventListener("wheel", onWheel, { passive: false });
    return () => viewport.removeEventListener("wheel", onWheel);
  }, [viewport]);

  useLayoutEffect(() => {
    const anchor = zoomAnchor.current;
    if (!viewport || !anchor) return;
    viewport.scrollLeft = anchor.graphX * zoom - anchor.x;
    viewport.scrollTop = anchor.graphY * zoom - anchor.y;
    zoomAnchor.current = null;
  }, [viewport, zoom]);

  const load = useCallback(async (agent: string) => {
    if (!agent) {
      setSnapshot(null);
      setError("DAG view is scoped to an agent.");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const next = await gatewayRequest("GET", `/agents/${agent}/dag`) as DagSnapshot;
      setSnapshot(next);
      setSelected((current) => next.nodes.some((node) => node.id === current) ? current : next.nodes[0]?.id ?? null);
    } catch (cause) {
      setSnapshot(null);
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(activeAgent);
    const timer = window.setInterval(() => void load(activeAgent), 5000);
    return () => window.clearInterval(timer);
  }, [activeAgent, load]);

  const layout = useMemo(() => {
    if (!snapshot) return null;
    const levels = calculateLevels(snapshot);
    const maxLevel = Math.max(0, ...levels.values());
    const columns = new Map<number, string[]>();
    for (const node of snapshot.nodes) {
      const level = levels.get(node.id) ?? 0;
      const column = columns.get(level) ?? [];
      column.push(node.id);
      columns.set(level, column);
    }
    const positions = new Map<string, { x: number; y: number }>();
    const rows = Math.max(1, ...Array.from(columns.values(), ids => ids.length));
    const height = Math.max(280, rows * 100 + 56);
    for (const [level, ids] of columns) {
      ids.forEach((id, row) => positions.set(id, { x: 28 + level * 270, y: 28 + row * 100 + (rows - ids.length) * 50 }));
    }
    return {
      levels,
      positions,
      width: Math.max(500, (maxLevel + 1) * 270 + 48),
      height,
    };
  }, [snapshot]);

  const selectedNode = snapshot?.nodes.find((node) => node.id === selected) ?? null;
  const changeZoom = (value: number) => {
    const next = Math.max(0.25, Math.min(3, value));
    if (viewport && next !== zoomRef.current) {
      const x = viewport.clientWidth / 2, y = viewport.clientHeight / 2;
      zoomAnchor.current = { x, y, graphX: (viewport.scrollLeft + x) / zoomRef.current, graphY: (viewport.scrollTop + y) / zoomRef.current };
    }
    zoomRef.current = next;
    setZoom(next);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div className="flex min-h-10 flex-wrap items-center justify-between gap-2 border-b px-3 py-1.5 text-xs text-muted-foreground">
        <span className="flex items-center gap-2"><GitBranch className="size-4" />
          {snapshot
            ? `${snapshot.nodes.length} nodes · ${snapshot.edges.length} edges · ${snapshot.nodes.filter((node) => dagStatus(node.status) === "running").length} running · ${snapshot.nodes.filter((node) => dagStatus(node.status) === "failed").length} failed`
            : `agent ${activeAgent || "—"}`}
        </span>
        <div className="flex items-center gap-1"><Button size="icon" variant="ghost" className="size-7" aria-label="Zoom out" onClick={() => changeZoom(zoom / 1.2)}><ZoomOut className="size-4" /></Button><button type="button" className="min-w-12 rounded px-1 py-1 tabular-nums hover:bg-accent" title="Reset zoom" onClick={() => changeZoom(1)}>{Math.round(zoom * 100)}%</button><Button size="icon" variant="ghost" className="size-7" aria-label="Zoom in" onClick={() => changeZoom(zoom * 1.2)}><ZoomIn className="size-4" /></Button><span className="mx-1 h-4 w-px bg-border" /><Button size="icon" variant="ghost" className="size-7" aria-label="Refresh DAG" disabled={loading || !activeAgent} onClick={() => void load(activeAgent)}><RefreshCw className={cn("size-4", loading && "animate-spin")} /></Button></div>
      </div>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">{error}</div>}
      <div className="relative min-h-0 flex-1">
      {snapshot && layout && snapshot.nodes.length > 0 ? (
          <ScrollArea className="h-full" scrollOrientation="both" viewportRef={setViewport} viewportLabel="DAG graph" viewportTabIndex={0}>
          <div style={{ width: layout.width * zoom, height: layout.height * zoom }}>
          <div className="relative origin-top-left" style={{ width: layout.width, height: layout.height, transform: `scale(${zoom})`, backgroundImage: "radial-gradient(var(--border) 1px, transparent 1px)", backgroundSize: "20px 20px" }}>
            <svg width={layout.width} height={layout.height} className="absolute inset-0 pointer-events-none">
              {snapshot.edges.map((edge) => {
                const from = layout.positions.get(edge.from);
                const to = layout.positions.get(edge.to);
                if (!from || !to) return null;
                const selectedEdge = selected === edge.from || selected === edge.to;
                const start = from.x + 210;
                const bend = (to.x - start) / 2;
                return (
                  <path
                    key={`${edge.from}:${edge.from_port}:${edge.to}:${edge.to_port}`}
                    d={`M ${start} ${from.y + 36} C ${start + bend} ${from.y + 36}, ${to.x - bend} ${to.y + 36}, ${to.x} ${to.y + 36}`}
                    className={cn("fill-none stroke-2", selectedEdge ? "stroke-primary" : "stroke-border")}
                  />
                );
              })}
            </svg>
            {snapshot.nodes.map((node) => {
              const position = layout.positions.get(node.id);
              if (!position) return null;
              const status = dagStatus(node.status);
              const colors = statusColors[status] ?? statusColors.pending;
              return (
                <button
                  key={node.id}
                  type="button"
                  onClick={() => setSelected(node.id)}
                  aria-pressed={selected === node.id}
                  style={{ left: position.x, top: position.y }}
                  className={cn(
                    "absolute flex h-[72px] w-[210px] flex-col items-start gap-1 overflow-hidden rounded-md border border-l-4 bg-card bg-clip-padding p-2 text-left shadow-sm transition-colors hover:brightness-95 dark:hover:brightness-125 focus-visible:outline-2 focus-visible:outline-ring",
                    colors.node,
                    selected === node.id && "ring-1 ring-primary",
                  )}
                >
                  <strong className="w-full truncate text-sm">{node.id}</strong>
                  <span className="w-full truncate text-xs text-muted-foreground">{node.kind}</span>
                  <span className="flex items-center gap-1.5 text-[10px]"><span className={cn("rounded px-1.5 font-medium leading-4", colors.badge)}>{status}</span>{node.dirty && <span className="text-muted-foreground">dirty</span>}</span>
                </button>
              );
            })}
          </div>
          </div>
          </ScrollArea>
      ) : <div className="grid h-full place-items-center p-6 text-center text-sm text-muted-foreground">{loading ? "Loading DAG snapshot…" : error ? "Could not load DAG." : "No DAG nodes"}</div>}
      {snapshot && snapshot.nodes.length > 0 && <div className="pointer-events-none absolute bottom-2 left-3 rounded bg-background/90 px-2 py-1 text-[11px] text-muted-foreground">Ctrl + scroll to zoom</div>}
      </div>
      <Card className="max-h-40 overflow-auto rounded-none border-x-0 border-b-0 bg-muted/20 p-3 shadow-none">
        {selectedNode ? (
          <pre className="text-xs whitespace-pre-wrap">
            {[
              `ID: ${selectedNode.id}`,
              `Kind: ${selectedNode.kind}`,
              `Status: ${dagStatus(selectedNode.status)}`,
              `Cache: ${selectedNode.dirty ? "dirty" : "cached"}`,
              `Inputs: ${selectedNode.inputs.map((port) => `[${port.index}] ${port.label || port.data_type}`).join(", ") || "—"}`,
              `Outputs: ${selectedNode.outputs.map((port) => `[${port.index}] ${port.label || port.data_type}`).join(", ") || "—"}`,
            ].join("\n")}
          </pre>
        ) : <div className="text-sm text-muted-foreground">No node selected</div>}
      </Card>
    </div>
  );
}
