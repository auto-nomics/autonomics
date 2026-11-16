/**
 * Agent inspector: overview card, DAG minimap, plan, view toggles and
 * actions. Relocated verbatim (layout-wise) from the retired AgentView;
 * it now lives as the second column of the auxiliary agent pane.
 */

import { DagMinimap } from "@/features/system/dag-minimap";
import { BrowserMonitor } from "./browser-monitor";
import { Fragment } from "react";
import { Check, Circle, CircleDashed, Copy, History, RefreshCw, X } from "lucide-react";
import type { AgentInfo, ChatItem } from "@/types";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import type { DisplaySettings } from "./agent";
import { statusLabel } from "./agent";
import { TokenUsageBadge } from "./token-usage";

function planStepStatus(status: unknown) {
  const value = String(status ?? "").replace(/[_\s-]/g, "").toLowerCase();
  if (value === "completed") return { label: "Completed", Icon: Check, style: "text-emerald-600 dark:text-emerald-400", completed: true, active: false };
  if (value === "inprogress") return { label: "In progress", Icon: CircleDashed, style: "text-primary", completed: false, active: true };
  if (value === "cancelled" || value === "canceled") return { label: "Cancelled", Icon: X, style: "text-muted-foreground", completed: false, active: false };
  return { label: "Pending", Icon: Circle, style: "text-muted-foreground", completed: false, active: false };
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
  const planSteps: { step: string; status: string }[] = plan?.update?.plan ?? [];
  const completedSteps = planSteps.filter(step => planStepStatus(step.status).completed).length;
  const explanation = plan?.update?.explanation;

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
                <TokenUsageBadge usage={usage} className="shrink-0" />
              </div>
            )}
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
        <h2 className="mb-1.5 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Browser</h2>
        {inspectorOpen && activeAgent ? <BrowserMonitor activeAgent={activeAgent} open={inspectorOpen} /> : <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No agent selected</div>}
      </section>

      <section>
        <div className="mb-1.5 flex items-center justify-between gap-2">
          <h2 className="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Plan</h2>
          {planSteps.length > 0 && <span className="text-[11px] tabular-nums text-muted-foreground" aria-live="polite">{completedSteps} / {planSteps.length} completed</span>}
        </div>
        {planSteps.length ? (
          <div className="overflow-hidden rounded-lg border bg-card">
            <div className="px-3 pt-3 pb-2">
              <div role="progressbar" aria-label="Plan completion" aria-valuemin={0} aria-valuemax={planSteps.length} aria-valuenow={completedSteps} className="h-1 overflow-hidden rounded-full bg-muted">
                <div className="h-full rounded-full bg-emerald-500 transition-[width] motion-reduce:transition-none" style={{ width: `${completedSteps / planSteps.length * 100}%` }} />
              </div>
            </div>
            {typeof explanation === "string" && explanation.trim() && (
              <p className="mx-3 mb-2 whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">{explanation}</p>
            )}
            <ol className="space-y-1 p-1.5 pt-0" aria-label="Plan steps">
              {planSteps.map((step, index) => {
                const state = planStepStatus(step.status);
                const Icon = state.Icon;
                return (
                  <li key={index} aria-current={state.active ? "step" : undefined} className={cn("flex items-start gap-2.5 rounded-md border border-transparent px-2 py-2 text-xs", state.active && "border-primary/20 bg-primary/5")}>
                    <span className={cn("mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full", state.style, state.active && "bg-primary/10", state.completed && "bg-emerald-500/10")}>
                      <Icon className="size-3.5" aria-hidden="true" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="mb-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10px]">
                        <span className="tabular-nums text-muted-foreground">Step {index + 1}</span>
                        <span className={state.style}>{state.label}</span>
                      </div>
                      <p className={cn("whitespace-pre-wrap leading-relaxed [overflow-wrap:anywhere]", state.completed && "text-muted-foreground", state.active && "font-medium")}>{step.step}</p>
                    </div>
                  </li>
                );
              })}
            </ol>
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
