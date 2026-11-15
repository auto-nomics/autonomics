import { useEffect, useRef, useState } from "react";
import { Search } from "lucide-react";
import { AgentStatusIcon, EFFORT_OPTIONS, effortLabel, leafName, modelShortLabel, statusLabel } from "@/features/agent";
import { ContextMenu, ContextMenuCheckboxItem, ContextMenuContent, ContextMenuItem, ContextMenuShortcut, ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger, ContextMenuTrigger } from "@/components/ui/context-menu";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import type { AgentRuntime } from "../runtime-context";
import { openAgentEditor } from "../workbench-store";

export function AgentListPanel({ runtime, active }: { runtime: AgentRuntime; active: boolean }) {
  const [query, setQuery] = useState("");
  const paths = [...new Set([...runtime.openTabs, ...runtime.agents.map(agent => agent.path)])];
  const shown = paths.filter(path => path.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));

  // Inline model/effort cells need the per-agent effort, which the runtime
  // only caches for agents with an opened workspace tab. Request each listed
  // path once per mount; the hover card keeps refetching on open so values
  // stay fresh. The requested-set guards the dep-less effect against retry
  // loops when the endpoint fails (the runtime swallows the error and the
  // cache entry stays undefined).
  const requestedEfforts = useRef(new Set<string>());
  useEffect(() => {
    for (const path of shown) {
      if (requestedEfforts.current.has(path) || runtime.agentEfforts[path] !== undefined) continue;
      requestedEfforts.current.add(path);
      void runtime.refreshAgentEffort(path);
    }
  });
  // The context menu's model submenu reads the shared runtime catalog, which
  // is normally fetched by the composer's switcher on first open. Load it
  // here too, on first right-click; a failure is surfaced as a disabled row.
  const [catalogError, setCatalogError] = useState(false);
  const loadCatalog = () => {
    runtime.loadModelCatalog().then(() => setCatalogError(false)).catch(() => setCatalogError(true));
  };

  return <div id="agent-sidebar-list" role="tabpanel" aria-label="Agent 列表" hidden={!active} className={cn("min-h-0 flex-1 flex-col", active ? "flex" : "hidden")}>
    <div className="relative mx-3 mt-3">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
      <Input aria-label="搜索 Agent" placeholder="搜索 Agent" className="h-8 pl-8 text-sm" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => { if (event.key === "Escape") setQuery(""); }} />
    </div>
    <div className="flex h-9 shrink-0 items-center border-b px-4 text-xs text-muted-foreground">{shown.length} 个 Agent</div>
    <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
      {shown.length === 0 ? <p className="px-2 py-3 text-xs text-muted-foreground">{query ? "没有匹配的 Agent。" : "暂无 Agent，可创建或恢复。"}</p> : <div role="list" aria-label="Agent 列表">
        {shown.map(path => {
          const status = runtime.statuses[path] ?? "idle";
          const modelSpec = runtime.agents.find(agent => agent.path === path)?.model;
          const modelShort = modelShortLabel(modelSpec);
          const effort = runtime.agentEfforts[path];
          const effortText = effort === undefined ? "…" : effort === null ? "不支持思考" : effortLabel(effort);
          const effortShort = effort === undefined ? "…" : effort === null ? "无" : effortLabel(effort);
          return <div role="listitem" key={path}><ContextMenu onOpenChange={open => { if (open) { loadCatalog(); void runtime.refreshAgentEffort(path); } }}><HoverCard onOpenChange={open => { if (open) void runtime.refreshAgentEffort(path); }}>
            <HoverCardTrigger delay={150} closeDelay={100} render={
              <ContextMenuTrigger render={
                <button type="button" aria-label={`打开 Agent ${path}（${statusLabel(status)}）`} aria-current={path === runtime.activeAgent ? "true" : undefined} className={cn("flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent hover:text-foreground", path === runtime.activeAgent && "bg-accent text-foreground")} onClick={() => { runtime.selectAgent(path); openAgentEditor(path); }}>
                  <AgentStatusIcon status={status} />
                  <span className="min-w-0 flex-1 truncate">{leafName(path)}</span>
                  <span className="flex shrink-0 items-center gap-1 text-[10px] tabular-nums text-muted-foreground/80">
                    <span className="max-w-24 truncate">{modelShort ?? "—"}</span>
                    <span aria-hidden>·</span>
                    <span>{effortShort}</span>
                  </span>
                </button>
              } />
            } />
            <HoverCardContent side="right" sideOffset={8} className="w-auto max-w-80 px-3 py-2">
              <p className="break-all font-mono text-xs">{path}</p>
              <p className="mt-1 flex items-center gap-1.5 text-[11px] text-muted-foreground"><AgentStatusIcon status={status} />{statusLabel(status)}</p>
              <div className="mt-1.5 space-y-0.5 border-t pt-1.5 text-[11px] text-muted-foreground">
                <p>模型：<span className="font-mono text-foreground">{modelSpec ?? "—"}</span></p>
                <p>推理力度：{effortText}</p>
              </div>
            </HoverCardContent>
          </HoverCard>
          <ContextMenuContent>
                <ContextMenuSub>
                  <ContextMenuSubTrigger>模型：{modelShort ?? "—"}</ContextMenuSubTrigger>
                  <ContextMenuSubContent className="max-h-72 overflow-y-auto">
                    {catalogError ? <ContextMenuItem disabled>模型目录加载失败</ContextMenuItem>
                      : !runtime.modelCatalog.length ? <ContextMenuItem disabled>模型目录为空，请在「设置 → 模型」中添加</ContextMenuItem>
                        : runtime.modelCatalog.map(model => {
                          const spec = `${model.provider_name}:${model.model_name}`;
                          return <ContextMenuCheckboxItem key={spec} checked={spec === modelSpec} closeOnClick onClick={() => { if (spec !== modelSpec) void runtime.setAgentModel(path, spec); }}>
                            <span className="min-w-0 flex-1 truncate">{model.model_name}</span>
                            <ContextMenuShortcut>{model.provider_name}</ContextMenuShortcut>
                          </ContextMenuCheckboxItem>;
                        })}
                  </ContextMenuSubContent>
                </ContextMenuSub>
                <ContextMenuSub>
                  <ContextMenuSubTrigger disabled={effort === null}>推理力度：{effortText}</ContextMenuSubTrigger>
                  <ContextMenuSubContent>
                    {effort === null ? <ContextMenuItem disabled>当前模型不支持思考</ContextMenuItem>
                      : EFFORT_OPTIONS.map(option => <ContextMenuCheckboxItem key={option.value} checked={effort === option.value} closeOnClick onClick={() => { if (effort !== option.value) void runtime.setAgentEffort(path, option.value); }}>
                        <span className="min-w-0 flex-1">{option.label}</span>
                        <ContextMenuShortcut>{option.hint}</ContextMenuShortcut>
                      </ContextMenuCheckboxItem>)}
                  </ContextMenuSubContent>
                </ContextMenuSub>
          </ContextMenuContent>
        </ContextMenu></div>;
        })}
      </div>}
    </ScrollArea>
  </div>;
}
