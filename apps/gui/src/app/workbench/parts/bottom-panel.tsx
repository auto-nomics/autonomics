/**
 * Bottom panel: the VSCode-style output area. Tabs: 事件流 (live gateway
 * frame log, delta frames elided) and 通知 (gateway notices — model
 * changes, ChatGPT login results, catalog fetches — previously unclaimed
 * by any view).
 */

import { X } from "lucide-react";
import type { FrameLogEntry, NoticeEntry } from "@/app/use-agent-runtime";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { setPanelTab, setPanelVisible, useWorkbenchState } from "../workbench-store";

function formatTime(time: number) {
  return new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function noticeSummary(entry: NoticeEntry): string {
  const data = entry.data as Record<string, unknown> | null | undefined;
  if (!data || typeof data !== "object") return String(entry.data ?? "");
  const parts = Object.entries(data).map(([key, value]) => {
    const rendered = typeof value === "object" && value !== null ? JSON.stringify(value) : String(value);
    return `${key}=${rendered}`;
  });
  return parts.join(" · ");
}

export function BottomPanel(props: { frameLog: FrameLogEntry[]; notices: NoticeEntry[] }) {
  const state = useWorkbenchState();
  const tab = state.panel.tab;

  return (
    <section aria-label="底部面板" className="flex h-full min-h-0 flex-col bg-background">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b px-1.5" role="tablist" aria-label="面板标签">
        {([
          ["events", "事件流"],
          ["notices", "通知"],
        ] as const).map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            aria-label={label}
            onClick={() => setPanelTab(id)}
            className={cn(
              "rounded-md px-2 py-0.5 text-xs",
              tab === id ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-accent/60",
            )}
          >
            {label}
            {id === "notices" && state.panel.unreadNotices > 0 && (
              <span className="ml-1 rounded-full bg-primary px-1 text-[10px] text-primary-foreground">
                {state.panel.unreadNotices}
              </span>
            )}
          </button>
        ))}
        <Button
          size="icon-xs"
          variant="ghost"
          className="ml-auto"
          aria-label="收起面板"
          title="收起面板(Ctrl+J)"
          onClick={() => setPanelVisible(false)}
        >
          <X />
        </Button>
      </div>
      {tab === "events" ? (
        <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 font-mono text-[11px]">
          {props.frameLog.map((entry, index) => (
            <div key={`${entry.seq}-${index}`} className="flex gap-2 py-px">
              <span className="shrink-0 text-muted-foreground/70">{formatTime(entry.time)}</span>
              <Badge variant="outline" className="h-4 shrink-0 px-1 text-[10px] uppercase">{entry.kind}</Badge>
              <span className="min-w-0 truncate text-muted-foreground">{entry.summary}</span>
            </div>
          ))}
          {!props.frameLog.length && (
            <div className="p-4 text-center font-sans text-xs text-muted-foreground">暂无事件 — 与 agent 交互后这里显示网关帧流</div>
          )}
        </ScrollArea>
      ) : (
        <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 font-sans text-xs">
          {props.notices.map((entry, index) => (
            <div key={`${entry.seq}-${index}`} className="flex gap-2 rounded-md border p-1.5">
              <span className="shrink-0 text-muted-foreground/70">{formatTime(entry.time)}</span>
              <span className="min-w-0 break-all">{noticeSummary(entry)}</span>
            </div>
          ))}
          {!props.notices.length && (
            <div className="p-4 text-center text-xs text-muted-foreground">暂无通知 — 模型切换、登录结果等网关通知会显示在这里</div>
          )}
        </ScrollArea>
      )}
    </section>
  );
}
