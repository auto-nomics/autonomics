import { useState } from "react";
import { History, List, Plus, Search, SlidersHorizontal } from "lucide-react";
import { AgentInspector, AgentStatusIcon, leafName, openAgentDialog, statusLabel } from "@/features/agent";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { useWorkbenchRuntime } from "../runtime-context";
import { openAgentEditor, openEditor, setContainerView, useWorkbenchState } from "../workbench-store";

export function AgentSidebar({ onSpawn }: { onSpawn: () => void }) {
  const runtime = useWorkbenchRuntime();
  const state = useWorkbenchState();
  const [query, setQuery] = useState("");
  const view = state.containerViews.agents === "overview" ? "overview" : "list";
  const paths = [...new Set([...runtime.openTabs, ...runtime.agents.map(agent => agent.path)])];
  const shown = paths.filter(path => path.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));

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
    </div>
    <div id="agent-sidebar-list" role="tabpanel" aria-label="Agent 列表" hidden={view !== "list"} className={cn("min-h-0 flex-1 flex-col", view === "list" ? "flex" : "hidden")}>
      <div className="relative mx-3 mt-3">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input aria-label="搜索 Agent" placeholder="搜索 Agent" className="h-8 pl-8 text-sm" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => { if (event.key === "Escape") setQuery(""); }} />
      </div>
      <div className="flex h-9 shrink-0 items-center border-b px-4 text-xs text-muted-foreground">{shown.length} 个 Agent</div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
        {shown.length === 0 ? <p className="px-2 py-3 text-xs text-muted-foreground">{query ? "没有匹配的 Agent。" : "暂无 Agent，可创建或恢复。"}</p> : <div role="list" aria-label="Agent 列表">
          {shown.map(path => {
            const status = runtime.statuses[path] ?? "idle";
            return <div role="listitem" key={path}><button type="button" title={path} aria-label={`打开 Agent ${path}`} aria-current={path === runtime.activeAgent ? "true" : undefined} className={cn("flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent hover:text-foreground", path === runtime.activeAgent && "bg-accent text-foreground")} onClick={() => { runtime.selectAgent(path); openAgentEditor(path); }}>
              <AgentStatusIcon status={status} />
              <span className="min-w-0 flex-1 truncate">{leafName(path)}</span><span className="shrink-0 text-[10px] text-muted-foreground">{statusLabel(status)}</span>
            </button></div>;
          })}
        </div>}
      </ScrollArea>
    </div>
    <div id="agent-sidebar-overview" role="tabpanel" aria-label="Agent 详情" hidden={view !== "overview"} className={cn("min-h-0 flex-1 flex-col overflow-hidden", view === "overview" ? "flex" : "hidden")}>
      <AgentInspector activeAgent={runtime.activeAgent} activeInfo={runtime.agents.find(agent => agent.path === runtime.activeAgent) ?? (runtime.agentModels[runtime.activeAgent] ? { path: runtime.activeAgent, agent_id: null, status: "Idle", ...runtime.agentModels[runtime.activeAgent] } : undefined)} status={runtime.statuses[runtime.activeAgent]} plan={runtime.plans[runtime.activeAgent] ?? null} usage={runtime.usage[runtime.activeAgent] ?? null} display={runtime.display} inspectorOpen={view === "overview"} onToggleDisplay={key => void runtime.toggleDisplay(key)} onOpenFlow={() => { if (runtime.activeAgent) openEditor({ kind: "flow", params: { agentPath: runtime.activeAgent } }); }}
          onOpenDag={() => { if (runtime.activeAgent) openEditor({ kind: "dag", params: { agentPath: runtime.activeAgent } }); }} onOpenCopy={() => openAgentDialog("copy")} onOpenHistory={() => openAgentDialog("history")} onRefreshHistory={runtime.refreshHistory} />
    </div>
  </div>;
}
