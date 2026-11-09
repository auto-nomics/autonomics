import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { useWorkbenchRuntime } from "./runtime-context";
import { registerWorkbenchView } from "./view-registry";
import { TerminalView } from "./parts/terminal-view";

function EventsView() {
  const runtime = useWorkbenchRuntime();
  return <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 font-mono text-[11px]">
    {runtime.frameLog.map((entry, index) => <div key={`${entry.seq}-${index}`} className="flex gap-2 py-px"><span className="shrink-0 text-muted-foreground/70">{new Date(entry.time).toLocaleTimeString()}</span><Badge variant="outline">{entry.kind}</Badge><span className="min-w-0 truncate">{entry.summary}</span></div>)}
    {!runtime.frameLog.length && <div className="p-4 text-xs text-muted-foreground">暂无事件 — 与 agent 交互后这里显示网关帧流</div>}
  </ScrollArea>;
}
function NoticesView() {
  const runtime = useWorkbenchRuntime();
  return <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 text-xs">
    {runtime.notices.map((entry, index) => <div key={`${entry.seq}-${index}`} className="flex gap-2 rounded-md border p-1.5"><span className="shrink-0 text-muted-foreground">{new Date(entry.time).toLocaleTimeString()}</span><span className="min-w-0 break-all">{JSON.stringify(entry.data)}</span></div>)}
    {!runtime.notices.length && <div className="p-4 text-muted-foreground">暂无通知 — 模型切换、登录结果等网关通知会显示在这里</div>}
  </ScrollArea>;
}
registerWorkbenchView({ id: "events", title: "事件流", defaultLocation: "panel", body: EventsView });
registerWorkbenchView({ id: "notices", title: "通知", defaultLocation: "panel", body: NoticesView });
registerWorkbenchView({ id: "terminal", title: "终端", defaultLocation: "panel", body: () => <TerminalView visible /> });
