import { ViewLocationMenu } from "../view-location-menu";
import { History, List, Plus, SlidersHorizontal } from "lucide-react";
import { AgentInspector, openAgentDialog } from "@/features/agent";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useWorkbenchRuntime } from "../runtime-context";
import { openEditor, setContainerView, useWorkbenchState } from "../workbench-store";
import { AgentListPanel } from "./agent-list-panel";

export function AgentSidebar({ onSpawn }: { onSpawn: () => void }) {
  const runtime = useWorkbenchRuntime();
  const state = useWorkbenchState();
  const view = state.containerViews.agents === "overview" ? "overview" : "list";

  return <div role="group" aria-label="Agent 工作区" className="flex min-h-0 flex-1 flex-col">
    <div className="flex h-9 shrink-0 items-center gap-0.5 border-b px-2">
      <div role="tablist" aria-label="Agent 侧栏视图" className="flex items-center gap-0.5">
        {([
          { id: "list", label: "Agent 列表", icon: List },
          { id: "overview", label: "Agent 详情", icon: SlidersHorizontal },
        ] as const).map(tab => <button key={tab.id} type="button" role="tab" aria-selected={view === tab.id} aria-controls={`agent-sidebar-${tab.id}`} tabIndex={view === tab.id ? 0 : -1} aria-label={tab.label} title={tab.label} className={cn("grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground", view === tab.id && "bg-secondary text-secondary-foreground")} onClick={() => setContainerView("agents", tab.id)} onKeyDown={event => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
          event.preventDefault();
          const next = tab.id === "list" ? "overview" : "list";
          setContainerView("agents", next);
          event.currentTarget.parentElement?.querySelector<HTMLButtonElement>(`[aria-controls="agent-sidebar-${next}"]`)?.focus();
        }}><tab.icon className="size-4" /></button>)}
      </div>
      <span className="flex-1" />
      <Button size="icon-sm" variant="ghost" aria-label="恢复 Agent" title="恢复 Agent" onClick={() => openAgentDialog("resume")}><History /></Button>
      <Button size="icon-sm" variant="ghost" aria-label="创建 Agent" title="创建 Agent" onClick={onSpawn}><Plus /></Button>
      <ViewLocationMenu id="agents" primaryOnly />
    </div>
    <AgentListPanel runtime={runtime} active={view === "list"} />
    <div id="agent-sidebar-overview" role="tabpanel" aria-label="Agent 详情" hidden={view !== "overview"} className={cn("min-h-0 flex-1 flex-col overflow-hidden", view === "overview" ? "flex" : "hidden")}>
      <AgentInspector activeAgent={runtime.activeAgent} activeInfo={runtime.agents.find(agent => agent.path === runtime.activeAgent) ?? (runtime.agentModels[runtime.activeAgent] ? { path: runtime.activeAgent, agent_id: null, status: "Idle", ...runtime.agentModels[runtime.activeAgent] } : undefined)} status={runtime.statuses[runtime.activeAgent]} plan={runtime.plans[runtime.activeAgent] ?? null} usage={runtime.usage[runtime.activeAgent] ?? null} display={runtime.display} inspectorOpen={view === "overview"} onToggleDisplay={key => void runtime.toggleDisplay(key)} onOpenFlow={() => { if (runtime.activeAgent) openEditor({ kind: "flow", params: { agentPath: runtime.activeAgent } }); }}
          onOpenDag={() => { if (runtime.activeAgent) openEditor({ kind: "dag", params: { agentPath: runtime.activeAgent } }); }} onOpenCopy={() => openAgentDialog("copy")} onOpenHistory={() => openAgentDialog("history")} onRefreshHistory={runtime.refreshHistory} />
    </div>
  </div>;
}
