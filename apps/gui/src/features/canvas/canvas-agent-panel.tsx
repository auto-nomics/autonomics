import { useState } from "react";
import type { LayoutPreference, CaptionStyle } from "./canvas-composition";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { CanvasAnnotations } from "./canvas-annotations";
import { useCanvasAgentHandoff, type CanvasAgentHandoffProps } from "./use-canvas-agent-handoff";
import { Button } from "@/components/ui/button";

export function CanvasAgentPanel({ projectId, canvasId, getRevision, getApi, ensureSaved, capturePreview, isDirty }: CanvasAgentHandoffProps) {
  const runtime = useWorkbenchRuntime();
  const [target, setTarget] = useState(runtime.activeAgent || runtime.agents[0]?.path || "");
  const [request, setRequest] = useState("");
  const [captionStyle, setCaptionStyle] = useState<CaptionStyle>("concise");
  const [layoutPreference, setLayoutPreference] = useState<LayoutPreference>("auto");
  const [audience, setAudience] = useState("");
  const { prepare, busy, error, targetAvailable } = useCanvasAgentHandoff({ projectId, canvasId, getRevision, getApi, ensureSaved, capturePreview, isDirty, target, audience, captionStyle, layoutPreference });
  return <aside aria-label="画布 Agent" className="w-72 shrink-0 overflow-auto border-l p-3 text-sm">
    <form className="flex flex-col gap-3" onSubmit={event => { event.preventDefault(); void prepare(request); }}>
      <p className="font-medium">与 Agent 一起绘图</p>
      <label>Agent<select aria-label="绘图 Agent" disabled={busy} className="mt-1 w-full rounded border bg-background p-1" value={target} onChange={event => setTarget(event.target.value)}><option value="">选择 Agent</option>{runtime.agents.map(agent => <option key={agent.path} value={agent.path}>{agent.path}</option>)}</select></label>
      <label>受众<input aria-label="绘图受众" disabled={busy} className="mt-1 w-full rounded border bg-background p-1" value={audience} onChange={event => setAudience(event.target.value)} placeholder="如：免疫学研究者" /></label>
      <label>构图偏好<select aria-label="构图偏好" disabled={busy} className="mt-1 w-full rounded border bg-background p-1" value={layoutPreference} onChange={event => setLayoutPreference(event.target.value as LayoutPreference)}><option value="auto">自动构图</option><option value="compact">紧凑分面</option><option value="horizontal">横向流程</option><option value="vertical">纵向流程</option></select></label>
      <label>图内说明风格<select aria-label="图内说明风格" disabled={busy} className="mt-1 w-full rounded border bg-background p-1" value={captionStyle} onChange={event => setCaptionStyle(event.target.value as CaptionStyle)}><option value="concise">简洁说明</option><option value="detailed">详细说明</option></select></label>
      <p className="text-xs text-muted-foreground">简洁说明保留范围与关键不确定性，完整引用随来源记录保存。详细说明可另列图注；已有科学文字须按本轮要求核对。</p>
      <label>绘图或修改需求<textarea aria-label="绘图需求" disabled={busy} className="mt-1 min-h-32 w-full rounded border bg-background p-2" value={request} onChange={event => setRequest(event.target.value)} placeholder="希望表达什么？选中的部分需要如何修改？" /></label>
      <p className="text-xs text-muted-foreground">将先保存画布，再把需求和选中对象放入 Agent 草稿，供你检查并发送。</p>
      <Button type="submit" disabled={busy || !targetAvailable || !request.trim()}>{busy ? "准备中…" : "交给 Agent"}</Button>
      {error && <p role="alert" className="text-destructive">{error}</p>}
    </form>
    <CanvasAnnotations preparing={busy} canPrepare={targetAvailable} onPrepare={annotation => prepare(annotation.note, annotation)} projectId={projectId} canvasId={canvasId} getApi={getApi} getRevision={getRevision} ensureSaved={ensureSaved} />
  </aside>;
}
