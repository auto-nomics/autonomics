/**
 * Status bar: gateway connection, the active agent + lifecycle tone, the
 * active model and the context token usage. VSCode-dense (24px row).
 */

import { cn } from "@/lib/utils";
import { AgentStatusIcon, leafName, statusLabel } from "@/features/agent";
import { TokenUsageBadge } from "@/features/agent/token-usage";
import type { AgentRuntime } from "../runtime-context";
import { useActiveEditorContext } from "../active-editor-context";

export function StatusBar({ runtime }: { runtime: AgentRuntime }) {
  const { editor, kind } = useActiveEditorContext();
  const activeInfo = runtime.agents.find((agent) => agent.path === runtime.activeAgent);
  const usage = runtime.usage[runtime.activeAgent] ?? null;
  const running = statusLabel(runtime.statuses[runtime.activeAgent]);

  return (
    <footer className="flex h-6 shrink-0 items-center gap-3 border-t bg-sidebar px-2 text-[11px] text-sidebar-foreground" aria-label="状态栏">
      <span
        className="flex shrink-0 items-center gap-1.5"
        title={runtime.connected ? "Gateway 已连接" : "Gateway 已断开"}
      >
        <span aria-hidden className={cn("size-2 rounded-full", runtime.connected ? "bg-primary" : "bg-destructive")} />
        {runtime.connected ? "Gateway 已连接" : "Gateway 已断开"}
      </span>
      {runtime.activeAgent && (
        <span className="flex min-w-0 items-center gap-1.5" title={runtime.activeAgent}>
          <AgentStatusIcon status={runtime.statuses[runtime.activeAgent]} />
          <span className="truncate">{leafName(runtime.activeAgent)}</span>
          <span className="shrink-0 text-muted-foreground">{running}</span>
        </span>
      )}
      <span className="hidden min-w-0 items-center gap-1 truncate text-muted-foreground sm:flex" title="活动模型">
        {activeInfo?.model ?? "no model"}
      </span>
      {editor && <span aria-label="活动编辑器" title={kind?.name?.(editor) ?? editor.id} className="min-w-0 truncate text-muted-foreground">{kind?.title(editor) ?? editor.kind}</span>}
      {usage ? <TokenUsageBadge usage={usage} className="ml-auto shrink-0 text-[10px]" /> : null}
    </footer>
  );
}
