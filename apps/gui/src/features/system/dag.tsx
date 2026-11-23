import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import type { DagNode, DagSnapshot } from "@/types";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
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
  const { t } = useTranslation("system");
  const [snapshot, setSnapshot] = useState<DagSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [viewportSize, setViewportSize] = useState({ width: 0, height: 0 });
  const [zoom, setZoom] = useState(1);
  const zoomRef = useRef(1);
  const zoomAnchor = useRef<{ x: number; y: number; graphX: number; graphY: number } | null>(null);

  useLayoutEffect(() => {
    if (!viewport) return;
    const updateSize = () => setViewportSize({ width: viewport.clientWidth, height: viewport.clientHeight });
    updateSize();
    const observer = new ResizeObserver(updateSize);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [viewport]);

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
      setError(t("dag.scopedToAgent"));
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
  const canvasWidth = Math.max(layout?.width ?? 0, viewportSize.width / zoom);
  const canvasHeight = Math.max(layout?.height ?? 0, viewportSize.height / zoom);
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
            ? t("dag.summary", {
                nodes: snapshot.nodes.length,
                edges: snapshot.edges.length,
                running: snapshot.nodes.filter((node) => dagStatus(node.status) === "running").length,
                failed: snapshot.nodes.filter((node) => dagStatus(node.status) === "failed").length,
              })
            : t("dag.agentLabel", { name: activeAgent || "—" })}
        </span>
        <div className="flex items-center gap-1"><Button size="icon" variant="ghost" className="size-7" aria-label={t("dag.zoomOut")} onClick={() => changeZoom(zoom / 1.2)}><ZoomOut className="size-4" /></Button><button type="button" className="min-w-12 rounded px-1 py-1 tabular-nums hover:bg-accent" title={t("dag.resetZoom")} onClick={() => changeZoom(1)}>{Math.round(zoom * 100)}%</button><Button size="icon" variant="ghost" className="size-7" aria-label={t("dag.zoomIn")} onClick={() => changeZoom(zoom * 1.2)}><ZoomIn className="size-4" /></Button><span className="mx-1 h-4 w-px bg-border" /><Button size="icon" variant="ghost" className="size-7" aria-label={t("dag.refreshDag")} disabled={loading || !activeAgent} onClick={() => void load(activeAgent)}><RefreshCw className={cn("size-4", loading && "animate-spin")} /></Button></div>
      </div>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">{error}</div>}
      <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
      <ResizablePanel defaultSize="65%" minSize="20%">
      <div className="relative h-full min-h-0">
      {snapshot && layout && snapshot.nodes.length > 0 ? (
          <ScrollArea className="h-full" scrollOrientation="both" viewportRef={setViewport} viewportLabel={t("dag.dagGraph")} viewportTabIndex={0}>
          <div style={{ width: canvasWidth * zoom, height: canvasHeight * zoom }}>
          <div className="relative origin-top-left" style={{ width: canvasWidth, height: canvasHeight, transform: `scale(${zoom})`, backgroundImage: "radial-gradient(var(--border) 1px, transparent 1px)", backgroundSize: "20px 20px" }}>
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
                  <span className="flex items-center gap-1.5 text-[10px]"><span className={cn("rounded px-1.5 font-medium leading-4", colors.badge)}>{status}</span>{node.dirty && <span className="text-muted-foreground">{t("dag.dirty")}</span>}</span>
                </button>
              );
            })}
          </div>
          </div>
          </ScrollArea>
      ) : <div className="grid h-full place-items-center p-6 text-center text-sm text-muted-foreground">{loading ? t("dag.loadingSnapshot") : error ? t("dag.loadFailed") : t("dag.noNodes")}</div>}
      {snapshot && snapshot.nodes.length > 0 && <div className="pointer-events-none absolute bottom-2 left-3 rounded bg-background/90 px-2 py-1 text-[11px] text-muted-foreground">{t("dag.ctrlScrollZoom")}</div>}
      </div>
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel defaultSize="35%" minSize="15%">
        <ScrollArea className="h-full" viewportLabel={t("dag.nodeDetails")}>
          {selectedNode && snapshot ? <DagNodeDetails key={selectedNode.id} node={selectedNode} snapshot={snapshot} onSelect={setSelected} /> : <div className="p-4 text-sm text-muted-foreground">{t("dag.noNodeSelected")}</div>}
        </ScrollArea>
      </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}


function DagNodeDetails({ node, snapshot, onSelect }: { node: DagNode; snapshot: DagSnapshot; onSelect: (id: string) => void }) {
  const { t } = useTranslation("system");
  const value = (data: unknown) => data == null ? "—" : typeof data === "string" ? data : JSON.stringify(data, null, 2);
  const time = (ms?: number) => ms ? new Date(ms).toLocaleString() : "—";
  const fields = (data: Record<string, unknown>) => <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-xs">{Object.entries(data).map(([label, data]) => <div key={label} className="contents"><dt className="text-muted-foreground">{label}</dt><dd className="min-w-0 whitespace-pre-wrap break-words font-mono">{value(data)}</dd></div>)}</dl>;
  return <section aria-label={t("dag.selectedNodeDetails")} className="space-y-4 p-4">
    <h2 className="break-all text-sm font-semibold">{node.id}</h2>
    {fields({ [t("dag.fields.kind")]: node.kind, [t("dag.fields.status")]: node.status, [t("dag.fields.cache")]: node.dirty ? t("dag.fields.cacheDirty") : t("dag.fields.cacheClean"), [t("dag.fields.runId")]: snapshot.run_id, [t("dag.fields.dagRunning")]: snapshot.running, [t("dag.fields.snapshotUpdated")]: time(snapshot.observed_at_ms) })}
    <div className="space-y-2"><h3 className="text-xs font-semibold">{t("dag.fields.execution")}</h3>{node.execution ? fields({ [t("dag.fields.phase")]: node.execution.phase, [t("dag.fields.progress")]: `${node.execution.current ?? "—"} / ${node.execution.total ?? "—"} ${node.execution.unit ?? ""}`, [t("dag.fields.message")]: node.execution.message, [t("dag.fields.phaseStarted")]: time(node.execution.phase_started_at_ms), [t("dag.fields.lastProgress")]: time(node.execution.last_work_progress_at_ms), [t("dag.fields.observedAt")]: time(node.execution.observed_at_ms), [t("dag.fields.worker")]: node.execution.worker }) : <p className="text-xs text-muted-foreground">{t("dag.fields.noExecution")}</p>}</div>
    <div className="grid gap-4 md:grid-cols-2">{(["inputs", "outputs"] as const).map(direction => <div key={direction} className="min-w-0 space-y-2"><h3 className="text-xs font-semibold">{direction === "inputs" ? t("dag.fields.inputs") : t("dag.fields.outputs")} ({node[direction].length})</h3>{node[direction].length ? <div className="overflow-x-auto rounded border"><table className="w-full text-left text-xs"><thead className="bg-muted/50"><tr><th className="p-2">{t("dag.fields.port")}</th><th className="p-2">{t("dag.fields.label")}</th><th className="p-2">{t("dag.fields.dataType")}</th></tr></thead><tbody>{node[direction].map(port => <tr key={port.index} className="border-t align-top"><td className="p-2 font-mono">{port.index}</td><td className="p-2 whitespace-pre-wrap break-all">{port.label || "—"}</td><td className="p-2 whitespace-pre-wrap break-all font-mono">{port.data_type}</td></tr>)}</tbody></table></div> : <p className="text-xs text-muted-foreground">{t("dag.fields.noPorts")}</p>}</div>)}</div>
    <div className="grid gap-4 md:grid-cols-2">{(["upstream", "downstream"] as const).map(direction => {
      const edges = snapshot.edges.filter(edge => direction === "upstream" ? edge.to === node.id : edge.from === node.id);
      return <div key={direction} className="min-w-0 space-y-2"><h3 className="text-xs font-semibold">{direction === "upstream" ? t("dag.fields.upstream") : t("dag.fields.downstream")} ({edges.length})</h3>{edges.length ? edges.map((edge, index) => {
        const target = direction === "upstream" ? edge.from : edge.to;
        return <button key={index} type="button" disabled={!snapshot.nodes.some(n => n.id === target)} onClick={() => onSelect(target)} className="block w-full rounded border p-2 text-left text-xs font-mono whitespace-pre-wrap break-all hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring" title={t("dag.fields.selectNode", { target })}>{edge.from} [{edge.from_port}] → {edge.to} [{edge.to_port}]</button>;
      }) : <p className="text-xs text-muted-foreground">{t("dag.fields.noConnections")}</p>}</div>;
    })}</div>
    <details><summary className="cursor-pointer text-xs font-semibold">{t("dag.fields.rawNodeJson")}</summary><pre className="mt-2 whitespace-pre-wrap break-words rounded bg-muted/50 p-3 text-xs">{JSON.stringify(node, null, 2)}</pre></details>
  </section>;
}
