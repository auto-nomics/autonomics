/**
 * HPC 任务面板:操作员视角的任务监控清单(跨目标的 MonitorHub 视图)。
 * 只读——watch/stop 绑定原始 agent 会话,由 agent 侧 hpc_jobs 工具完成;
 * 本面板呈现状态、调度器视图与轮询节奏。
 */
import { useCallback, useEffect, useState } from "react";
import { fetchHpcJobs } from "./api";
import type { MonitorView } from "./types";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

type BadgeVariant =
  | "default"
  | "secondary"
  | "destructive"
  | "outline"
  | null
  | undefined;

function stateTone(state: string): BadgeVariant {
  if (state === "completed") return "default";
  if (state === "failed" || state === "blocked") return "destructive";
  if (state === "running" || state === "pending") return "secondary";
  return "outline";
}

function formatTime(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleTimeString();
}

export function HpcJobsEditorView() {
  const [monitors, setMonitors] = useState<MonitorView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const reload = useCallback(async () => {
    setRefreshing(true);
    try {
      const response = await fetchHpcJobs();
      setMonitors(response.monitors);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void reload();
    const timer = window.setInterval(() => void reload(), 10_000);
    return () => window.clearInterval(timer);
  }, [reload]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-center justify-between gap-2 border-b px-3 py-2">
        <div>
          <h2 className="text-base font-semibold">HPC 任务</h2>
          <p className="text-xs text-muted-foreground">
            监控中的集群任务(10 秒自动刷新)。任务由 agent/DAG 运行提交;
            watch/停止监控绑定原始会话,在 agent 侧操作。
          </p>
        </div>
        <Button
          size="sm"
          variant="outline"
          disabled={refreshing}
          onClick={() => void reload()}
        >
          刷新
        </Button>
      </header>
      {error ? (
        <div className="border-b border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {error}
        </div>
      ) : null}
      <ScrollArea className="min-h-0 flex-1">
        {monitors === null ? (
          <p className="p-4 text-sm text-muted-foreground">加载中…</p>
        ) : monitors.length === 0 ? (
          <p className="p-4 text-sm text-muted-foreground">
            当前没有监控中的 HPC 任务。
          </p>
        ) : (
          <ul className="divide-y">
            {monitors.map((monitor) => (
              <li
                key={`${monitor.target_id ?? "legacy"}:${monitor.id}`}
                className="grid gap-1 px-3 py-2 text-sm"
              >
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant={stateTone(monitor.state)}>
                    {monitor.state}
                  </Badge>
                  <code className="text-xs">
                    {monitor.cluster}
                    {monitor.job_id ? ` · ${monitor.job_id}` : ""}
                  </code>
                  {monitor.scheduler_state ? (
                    <Badge variant="outline">{monitor.scheduler_state}</Badge>
                  ) : null}
                  <span className="flex-1" />
                  <span className="text-xs text-muted-foreground">
                    {monitor.target_id
                      ? `${monitor.target_id} r${monitor.target_revision}`
                      : "legacy"}
                  </span>
                </div>
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  <span title={monitor.submission_key}>
                    {monitor.agent_path}
                  </span>
                  <span>更新 {formatTime(monitor.updated_at)}</span>
                  <span>下次轮询 {formatTime(monitor.next_poll)}</span>
                </div>
                {monitor.last_error ? (
                  <p
                    className={cn(
                      "rounded border border-destructive/30 bg-destructive/5 px-2 py-1 text-xs",
                      "text-destructive",
                    )}
                  >
                    {monitor.last_error}
                  </p>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </ScrollArea>
    </div>
  );
}
