import { useCallback, useEffect, useMemo, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { DagSnapshot } from "@/types";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

function dagStatus(status: unknown) {
  if (typeof status === "string") return status.toLowerCase();
  if (status && typeof status === "object") return Object.keys(status)[0]?.toLowerCase() ?? "pending";
  return "pending";
}

function calculateLevels(snapshot: DagSnapshot) {
  const incoming = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const outgoing = new Map(snapshot.nodes.map((node) => [node.id, [] as string[]]));
  for (const edge of snapshot.edges) {
    incoming.set(edge.to, (incoming.get(edge.to) ?? 0) + 1);
    outgoing.get(edge.from)?.push(edge.to);
  }
  const levels = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const remaining = new Map(incoming);
  const queue = snapshot.nodes.filter((node) => (incoming.get(node.id) ?? 0) === 0).map((node) => node.id);
  while (queue.length) {
    const id = queue.shift()!;
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
  }, [activeAgent, load]);

  const layout = useMemo(() => {
    if (!snapshot) return null;
    const levels = calculateLevels(snapshot);
    const maxLevel = Math.max(0, ...levels.values());
    const columns = new Map<number, string[]>();
    for (const node of snapshot.nodes) {
      const level = levels.get(node.id) ?? 0;
      columns.set(level, [...(columns.get(level) ?? []), node.id]);
    }
    const positions = new Map<string, { x: number; y: number }>();
    let height = 280;
    for (const [level, ids] of columns) {
      ids.forEach((id, row) => positions.set(id, { x: 28 + level * 250, y: 28 + row * 100 }));
      height = Math.max(height, ids.length * 100 + 56);
    }
    return {
      levels,
      positions,
      width: Math.max(900, (maxLevel + 1) * 250 + 48),
      height,
    };
  }, [snapshot]);

  const selectedNode = snapshot?.nodes.find((node) => node.id === selected) ?? null;

  return (
    <div className="grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)_auto] gap-3">
      <div className="flex items-center justify-between text-sm text-muted-foreground">
        <span>
          {snapshot
            ? `${snapshot.nodes.length} nodes · ${snapshot.edges.length} edges · ${snapshot.nodes.filter((node) => dagStatus(node.status) === "running").length} running · ${snapshot.nodes.filter((node) => dagStatus(node.status) === "failed").length} failed`
            : `agent ${activeAgent || "—"}`}
        </span>
        <Button size="sm" variant="secondary" onClick={() => void load(activeAgent)}>Refresh</Button>
      </div>
      {loading && <Card className="p-3 text-sm text-muted-foreground">Loading DAG snapshot…</Card>}
      {error && <Card className="border-destructive/40 p-3 text-sm text-destructive">{error}</Card>}
      {snapshot && layout && (
        <Card className="min-h-0 p-0!">
          <ScrollArea className="h-full" scrollOrientation="both">
          <div className="relative" style={{ width: layout.width, height: layout.height }}>
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
              return (
                <button
                  key={node.id}
                  type="button"
                  onClick={() => setSelected(node.id)}
                  style={{ left: position.x, top: position.y }}
                  className={cn(
                    "absolute grid h-[72px] w-[210px] place-content-start gap-1 rounded-md border border-l-4 bg-card p-2 text-left hover:bg-accent",
                    selected === node.id && "border-primary shadow-[0_0_0_1px_var(--primary)]",
                    status === "pending" && "border-l-muted-foreground",
                    status === "ready" && "border-l-sky-400",
                    status === "running" && "border-l-amber-400",
                    status === "success" && "border-l-emerald-400",
                    status === "failed" && "border-l-destructive",
                    status === "cancelled" && "border-l-rose-300",
                  )}
                >
                  <strong className="truncate text-sm">{node.id}</strong>
                  <span className="truncate text-xs text-muted-foreground">{node.kind}</span>
                  <small className="truncate text-xs text-muted-foreground">{status}{node.dirty ? " · dirty" : ""}</small>
                </button>
              );
            })}
          </div>
          </ScrollArea>
        </Card>
      )}
      <Card className="p-4">
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
