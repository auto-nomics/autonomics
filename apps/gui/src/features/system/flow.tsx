import { useCallback, useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

/**
 * Stage-model flow view (Nextflow-style): ONE card per process regardless of
 * how many item instances it fans out to. The node-granular DAG view stays
 * for node-level debugging; this is the batch-workflow surface.
 */

type FlowProcess = {
  id: string;
  kind: string;
  error_strategy?: { mode?: string; max_retries?: number; retry_delay_ms?: number };
};

type FlowWire = {
  from: string;
  from_port: number;
  to: string;
  to_port: number;
  op: string;
};

type ProcessRunSummary = {
  total: number;
  succeeded: number;
  failed: number;
  skipped: number;
  cancelled: number;
  reused: number;
  ignored: number;
  summed_elapsed_ms: number;
};

export type FlowViewData = {
  flow: { processes: FlowProcess[]; wires: FlowWire[] };
  last_run: { ok: boolean; cancelled?: boolean; processes: Record<string, ProcessRunSummary> } | null;
};

export function FlowView({ activeAgent }: { activeAgent: string }) {
  const [view, setView] = useState<FlowViewData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      if (!activeAgent) return;
      setLoading(true);
      setError(null);
      try {
        const data = await gatewayRequest("GET", `/agents/${encodeURIComponent(activeAgent)}/flow`, undefined, { signal });
        setView(data as FlowViewData);
      } catch (err) {
        if ((err as Error).name === "AbortError") return;
        setError(String((err as Error).message ?? err));
      } finally {
        setLoading(false);
      }
    },
    [activeAgent],
  );

  useEffect(() => {
    const controller = new AbortController();
    void refresh(controller.signal);
    return () => controller.abort();
  }, [refresh]);

  if (!activeAgent) {
    return <div className="p-4 text-sm text-muted-foreground">No active agent — the flow is session state.</div>;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
        <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={loading}>
          {loading ? "Refreshing…" : "Refresh"}
        </Button>
        {view?.last_run && (
          <span
            className={cn(
              "rounded px-2 py-0.5 text-xs font-medium",
              view.last_run.ok ? "bg-emerald-500/15 text-emerald-600" : "bg-red-500/15 text-red-600",
            )}
          >
            last run {view.last_run.cancelled ? "cancelled" : view.last_run.ok ? "ok" : "failed"}
          </span>
        )}
        <span className="text-xs text-muted-foreground">
          process view — instances are run-scoped and do not expand this graph
        </span>
      </div>

      {error && <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-sm text-red-600">{error}</div>}

      {!error && !view && <div className="p-2 text-sm text-muted-foreground">loading flow…</div>}

      {view?.flow.processes.length === 0 && (
        <div className="p-2 text-sm text-muted-foreground">
          no flow set for this session — the agent builds one with set_flow
        </div>
      )}

      {view && view.flow.processes.length > 0 && (
        <ScrollArea className="min-h-0 flex-1">
          <div className="flex flex-col gap-2 pb-4">
            {view.flow.processes.map((process) => {
              const run = view.last_run?.processes[process.id] ?? null;
              const failed = run ? run.failed + run.cancelled > 0 : false;
              return (
                <Card key={process.id} className="p-3">
                  <div className="flex items-baseline justify-between gap-2">
                    <div className="flex items-baseline gap-2">
                      <span className="font-semibold">{process.id}</span>
                      <span className="text-xs text-muted-foreground">{process.kind}</span>
                      {process.error_strategy?.mode && process.error_strategy.mode !== "fail" && (
                        <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[11px] text-amber-600">
                          {process.error_strategy.mode}
                          {process.error_strategy.mode === "retry" && process.error_strategy.max_retries
                            ? ` ×${process.error_strategy.max_retries}`
                            : ""}
                        </span>
                      )}
                    </div>
                  </div>
                  {run && (
                    <div className={cn("mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs", failed ? "text-red-600" : "text-emerald-600")}>
                      <span>{run.total} total</span>
                      <span>{run.succeeded} ok</span>
                      <span>{run.failed} failed</span>
                      <span>{run.skipped} skipped</span>
                      <span>{run.reused} reused</span>
                      {run.ignored > 0 && <span>{run.ignored} ignored</span>}
                      <span className="text-muted-foreground">Σ {Math.round(run.summed_elapsed_ms / 100) / 10}s</span>
                    </div>
                  )}
                </Card>
              );
            })}

            {view.flow.wires.length > 0 && (
              <Card className="p-3">
                <div className="mb-1 text-xs font-medium text-muted-foreground">wires</div>
                <div className="flex flex-col gap-0.5 font-mono text-xs">
                  {view.flow.wires.map((wire, index) => (
                    <div key={index}>
                      {wire.from}:{wire.from_port} —{wire.op}→ {wire.to}:{wire.to_port}
                    </div>
                  ))}
                </div>
              </Card>
            )}
          </div>
        </ScrollArea>
      )}
    </div>
  );
}
