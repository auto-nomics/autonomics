/**
 * Agent inspector: overview card, DAG minimap, plan, view toggles and
 * actions. Relocated verbatim (layout-wise) from the retired AgentView;
 * it now lives as the second column of the auxiliary agent pane.
 */

import { DagMinimap } from "@/features/system/dag-minimap";
import { Fragment } from "react";
import { Copy, History, RefreshCw } from "lucide-react";
import type { AgentInfo, ChatItem } from "@/types";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import type { DisplaySettings } from "./agent";
import { statusLabel } from "./agent";

function planStepIcon(status: unknown) {
  const value = String(status ?? "");
  if (value === "Completed") return <span className="text-emerald-500">✓</span>;
  if (value === "Cancelled") return <span className="text-destructive">✗</span>;
  if (value === "InProgress") return <span className="animate-pulse text-sky-400">◐</span>;
  return <span className="text-muted-foreground">•</span>;
}

export type AgentInspectorProps = {
  activeAgent: string;
  activeInfo: AgentInfo | undefined;
  /** Lifecycle label for the active agent (statuses[activeAgent]). */
  status?: string;
  plan: { revision: number; update: any } | null;
  messages?: ChatItem[];
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  /** Whether the inspector panel is visibly open (gates the minimap). */
  inspectorOpen: boolean;
  onToggleDisplay: (key: keyof DisplaySettings) => void;
  onOpenDag: () => void;
  onOpenFlow: () => void;
  onOpenCopy: () => void;
  onOpenHistory: () => void;
  onRefreshHistory: () => void;
};

export function AgentInspector(props: AgentInspectorProps) {
  const { activeAgent, activeInfo, plan, usage, display, inspectorOpen } = props;
  const running = statusLabel(props.status);
  const planSteps: any[] = plan?.update?.plan ?? [];

  return (
    <ScrollArea className="min-h-0 flex-1" contentClassName="space-y-4 p-3">
      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Overview</h2>
        <Card className="p-2.5 text-xs">
          <div className="truncate" title={activeAgent}>{activeAgent || "No agent"}</div>
          <div className="mt-1.5 space-y-1 text-muted-foreground">
            <div className="flex justify-between gap-2">
              <span className="shrink-0">Model</span>
              <span className="truncate">{activeInfo?.model ?? "—"}</span>
            </div>
            <div className="flex justify-between gap-2">
              <span className="shrink-0">Status</span>
              <span className="truncate">{activeAgent ? running : "—"}</span>
            </div>
            {usage && (
              <div className="flex justify-between gap-2">
                <span className="shrink-0">Tokens</span>
                <span className="truncate">{usage.input} in / {usage.output} out</span>
              </div>
            )}
            {usage?.cache ? (
              <div className="flex justify-between gap-2">
                <span className="shrink-0">Cache</span>
                <span className="truncate">{usage.cache}</span>
              </div>
            ) : null}
          </div>
        </Card>
      </section>

      <section>
        <div className="mb-1.5 flex items-center justify-between">
          <h2 className="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">DAG / Flow</h2>
          <div className="flex gap-1">
            <Button size="xs" variant="ghost" disabled={!activeAgent} onClick={props.onOpenDag}>Nodes</Button>
            <Button size="xs" variant="ghost" disabled={!activeAgent} onClick={props.onOpenFlow}>Flow</Button>
          </div>
        </div>
        {inspectorOpen && activeAgent ? <DagMinimap key={activeAgent} activeAgent={activeAgent} /> : <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No agent selected</div>}
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Plan</h2>
        {planSteps.length ? (
          <div className="space-y-1">
            {planSteps.map((step, index) => (
              <div key={index} className="flex items-start gap-2 rounded-md border bg-card p-1.5 text-xs">
                <span className="mt-px shrink-0">{planStepIcon(step.status)}</span>
                <span className={cn("min-w-0", step.status === "Completed" && "text-muted-foreground line-through")}>
                  {step.step}
                </span>
              </div>
            ))}
          </div>
        ) : (
          <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No active plan</div>
        )}
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">View</h2>
        <Card size="sm" className="gap-0 py-0">
          <CardContent className="p-0">
            {([
              ["collapse_thinking", "Collapse thinking"],
              ["collapse_tool_calls", "Collapse tool calls"],
              ["collapse_tool_results", "Collapse tool results"],
            ] as const).map(([key, label], index) => (
              <Fragment key={key}>
                {index > 0 && <Separator />}
                <label className="flex w-full cursor-pointer items-center justify-between gap-2 px-2 py-1.5 text-sm hover:bg-accent">
                  <span>{label}</span>
                  <Switch size="sm" checked={display[key]} onCheckedChange={() => props.onToggleDisplay(key)} />
                </label>
              </Fragment>
            ))}
          </CardContent>
        </Card>
      </section>

      <section>
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Actions</h2>
        <Card size="sm" className="gap-0 py-0">
          <CardContent className="p-1.5">
            <div className="grid grid-cols-2 gap-1.5">
              {/* Cancel(终止)与 Compact(压缩)已移入 composer:
                  终止合并进发送按钮的运行态,压缩是 composer 图标。 */}
              <Button size="xs" variant="secondary" onClick={props.onOpenCopy}><Copy />Copy</Button>
              <Button size="xs" variant="secondary" onClick={props.onOpenHistory}><History />History</Button>
              <Button size="xs" variant="secondary" onClick={props.onRefreshHistory}><RefreshCw />Refresh</Button>
            </div>
          </CardContent>
        </Card>
      </section>
    </ScrollArea>
  );
}
