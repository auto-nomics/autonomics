import { useEffect, useRef, useState } from "react";
import { Check, Circle, CircleAlert, LoaderCircle } from "lucide-react";
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress, ProgressLabel, ProgressValue } from "@/components/ui/progress";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { cn } from "@/lib/utils";
import { clearEnrollment, type EnrollmentRun } from "./enrollment";

const labels: Record<string, string> = {
  validate: "检查配置", connection: "准备连接", authenticate: "SSH 身份认证",
  prepare_directory: "准备安装目录", check_python: "检查 Python",
  upload: "上传 worker", commit_upload: "提交安装文件", chmod: "设置执行权限",
  probe_worker: "校验 worker", worker_profile: "生成 worker 配置",
  data_source: "挂载数据源", save_target: "保存计算目标", select_target: "选择计算目标",
};
const bytes = (value: number) => `${(value / 1024 / 1024).toFixed(1)} MB`;

export function EnrollmentProgressPanel({ run }: { run: EnrollmentRun }) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    panel.current?.scrollIntoView({ behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "nearest" });
  }, [run.startedAt]);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    setNow(Date.now());
    if (run.status !== "running") return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [run.startedAt, run.status]);
  const elapsed = Math.max(0, Math.floor(((run.endedAt ?? now) - run.startedAt) / 1000));
  const waiting = run.status === "running" && now - (run.events.at(-1)?.at ?? run.startedAt) >= 15_000;
  const currentIndex = Math.max(0, run.steps.indexOf(run.stage));
  const percent = run.total && run.uploaded !== undefined ? Math.min(100, Math.floor(run.uploaded / run.total * 100)) : null;
  const currentLabel = labels[run.stage] ?? run.stage;
  return (
    <Card ref={panel} size="sm" role="region" aria-label="计算目标安装进度" data-enrollment-state={run.status}>
      <CardHeader>
        <CardTitle>{run.targetId} · 安装进度</CardTitle>
        <CardDescription>已用时 {Math.floor(elapsed / 60)} 分 {elapsed % 60} 秒</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <div role="status" aria-live="polite" className="flex flex-wrap items-center gap-2">
          <Badge variant={run.status === "error" || run.status === "unknown" ? "destructive" : "secondary"}>
            {run.status === "success" ? "安装完成" : run.status === "error" ? "安装失败" : run.status === "unknown" ? "状态待确认" : "安装中"}
          </Badge>
          <span className="text-sm">{run.status === "success" ? "计算目标已保存" : currentLabel}</span>
          {waiting ? <span className="text-xs text-muted-foreground">等待节点响应，连接仍在监听…</span> : null}
        </div>
        {percent !== null ? (
          <div className="grid gap-1">
            <Progress value={percent} aria-label="worker 上传进度">
              <ProgressLabel>worker 上传</ProgressLabel>
              <ProgressValue>{() => `${percent}%`}</ProgressValue>
            </Progress>
            <p className="text-xs text-muted-foreground tabular-nums">
              {bytes(run.uploaded ?? 0)} / {bytes(run.total!)}
              {percent === 100 && run.status === "running" ? " · 上传已完成，正在完成后续配置" : ""}
            </p>
          </div>
        ) : null}
        <ol aria-label="安装步骤" className="grid gap-2 sm:grid-cols-2">
          {run.steps.map((stage, index) => {
            const done = run.status === "success" || index < currentIndex;
            const active = index === currentIndex && run.status !== "success";
            const failed = active && (run.status === "error" || run.status === "unknown");
            const Icon = done ? Check : failed ? CircleAlert : active ? LoaderCircle : Circle;
            return (
              <li key={stage} aria-current={active ? "step" : undefined}
                className={cn("flex items-center gap-2 text-xs", !active && !done && "text-muted-foreground", failed && "text-destructive")}>
                <Icon aria-hidden className={cn("size-4 shrink-0", active && !failed && "motion-safe:animate-spin")} />
                <span>{labels[stage] ?? stage}</span>
                <span className="sr-only">{done ? "已完成" : failed ? "失败" : active ? "进行中" : "等待中"}</span>
              </li>
            );
          })}
        </ol>
        {run.error ? (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertTitle>{run.status === "unknown" ? "进度连接已中断" : `${currentLabel}未完成`}</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap break-words">{run.error}</AlertDescription>
          </Alert>
        ) : null}
        <details className="text-xs">
          <summary className="cursor-pointer text-muted-foreground">详细日志（{run.events.length} 条，保留最近 500 条）</summary>
          <pre role="log" aria-label="安装详细日志" className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono">
            {run.events.map((event, index) => <span key={index} className="block">
              [{Math.max(0, Math.floor((event.at - run.startedAt) / 1000))}s] [{labels[event.stage] ?? event.stage}] {event.message}
            </span>)}
          </pre>
        </details>
      </CardContent>
      <CardFooter className="flex flex-wrap justify-between gap-2">
        <p className="text-xs text-muted-foreground">{run.status === "running"
          ? "切换页面不会中断安装；关闭或刷新窗口可能失去进度连接。"
          : run.status === "unknown" ? "后台可能仍在安装，请先刷新目标状态，避免重复部署。" : run.status === "error" ? "已完成的步骤会保留。重试前请确认目标的当前状态。" : "可在目标列表中查看配置并执行就绪检查。"}</p>
        {run.status !== "running" ? <Button size="sm" variant="ghost" onClick={clearEnrollment}>收起进度</Button> : null}
      </CardFooter>
    </Card>
  );
}
