import { useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import type { Planning } from "./canvas-planning";
import type { MarkupSummary } from "./canvas-markup-render";
import { createSourceManifest } from "./canvas-source-manifest";
import { CanvasAnnotations, type Annotation } from "./canvas-annotations";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";

export function CanvasAgentPanel({ projectId, canvasId, getRevision, getApi, ensureSaved, capturePreview, isDirty }: {
  projectId: string; canvasId: string; getRevision: () => number | undefined;
  getApi: () => ExcalidrawImperativeAPI | null; ensureSaved: () => Promise<boolean>; capturePreview: () => Promise<boolean>; isDirty: () => boolean;
}) {
  const runtime = useWorkbenchRuntime();
  const [target, setTarget] = useState(runtime.activeAgent || runtime.agents[0]?.path || "");
  const [request, setRequest] = useState("");
  const [audience, setAudience] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <aside aria-label="画布 Agent" className="w-72 shrink-0 overflow-auto border-l p-3 text-sm">
    <form className="flex flex-col gap-3" onSubmit={async event => {
      event.preventDefault(); if (busy || !target || !request.trim()) return;
      setBusy(true); setError("");
      try {
        if (!await ensureSaved()) throw new Error("画布未能保存，请先解决保存错误。");
        const api = getApi(); const revision = getRevision();
        if (!api || revision === undefined) throw new Error("画布尚未就绪");
        const previewAvailable = api.getSceneElements().length > 0 ? await capturePreview() : false;
        if (api.getSceneElements().length > 0 && !previewAvailable) throw new Error("画布预览未能生成，请重试。");
        const observed = serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
        const selected = api.getAppState().selectedElementIds;
        const objects = api.getSceneElements().filter(element => selected[element.id]).slice(0, 100).map(element => ({ id: element.id, type: element.type, x: element.x, y: element.y, width: element.width, height: element.height, ...("text" in element ? { text: element.text.slice(0, 1000) } : {}) }));
        const annotations = (await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/annotations`) as Annotation[]).filter(note => note.status === "open").slice(0, 30);
        const markups = (await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/markups`) as MarkupSummary[]).filter(item => item.status === "open");
        const savedPlan = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/planning`) as Planning | null;
        const planning = savedPlan ? { version: savedPlan.version, based_on_revision: savedPlan.based_on_revision, status: savedPlan.status, issues: savedPlan.issues, proposal_id: savedPlan.proposal_id } : null;
        const manifest = await createSourceManifest({ scene: observed, title: "", project_id: projectId, figure_id: canvasId, based_on_revision: revision, unsaved_changes: false });
        const assetSources = { total: manifest.assets.length, truncated: manifest.assets.length > 30, assets: [...manifest.assets].sort((a,b) => Number(Boolean(selected[b.element_id ?? ""] || selected[b.semantic_anchor_id ?? ""])) - Number(Boolean(selected[a.element_id ?? ""] || selected[a.semantic_anchor_id ?? ""]))).slice(0, 30).map(asset => ({ element_id: asset.element_id, semantic_anchor_id: asset.semantic_anchor_id, mime_type: asset.mime_type, provider: asset.provider, name: asset.name, authors: asset.authors, licenses: asset.licenses, license_urls: asset.license_urls, source_urls: asset.source_urls, actual_sha256: asset.actual_sha256, hash_matches_recorded: asset.hash_matches_recorded, modifications: asset.modifications.slice(-3), missing_fields: asset.missing_fields })) };
        if (isDirty() || getRevision() !== revision || serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") !== observed) throw new Error("交接期间画布发生变化，请保存后重试。");
        const prompt = [
          "请作为科研绘图助手处理下面的画布需求。先用 visualization read 获取当前文档与版本；若 preview_available 为 true，使用 read_preview 查看同版本整图，选中参考图可用 read_image 查看；优先通过 propose 提交可在画布中预览的局部修改提案，保留无关元素。若 markups 非空，使用 read_markup 和对应 markup_id 读取带标记的审阅图与说明；划除、补画、强调是审阅意图，不是正式元素。核对 at_revision，历史标记须与当前图稿比对后再修改。需要素材时可使用 bioart（PNG）或 bioicons（SVG）；先预览并核对逐素材作者、许可与科学含义，不要把通用图标当作特定分子的证据。asset_sources 是同版本素材来源摘要；保留逐素材作者、许可及修改记录，缺失信息不得猜测补写。需要规划新图时使用 read_plan/save_plan 保存图意、受众、组件、关系、假设和待澄清问题，向用户提问并保留已填写的答案；不要猜测缺失的科学机制。已有计划时用 read_plan 查看完整内容，使用 planning_version 和当前 expected_revision 更新计划；待澄清项完成并与用户核对后使用 sketch_plan 生成提案，或提示用户打开绘图计划进行编辑和生成。仅在没有已保存计划且用户已给出完整结构时使用直接 sketch。完成后说明修改和需要用户核对的科学假设，并提示点击查看 Agent 提案进行预览和采用。",
          JSON.stringify({ project_id: projectId, figure_id: canvasId, observed_revision: revision, audience: audience.trim(), request: request.trim(), selected_objects: objects, annotations, markups, planning, asset_sources: assetSources, preview_available: previewAvailable }, null, 2),
        ].join("\n\n");
        const previous = runtime.drafts[target] || "";
        runtime.setDraftFor(target, previous ? `${previous}\n\n${prompt}` : prompt);
        runtime.selectAgent(target); openAgentEditor(target);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>
      <p className="font-medium">与 Agent 一起绘图</p>
      <label>Agent<select aria-label="绘图 Agent" className="mt-1 w-full rounded border bg-background p-1" value={target} onChange={event => setTarget(event.target.value)}><option value="">选择 Agent</option>{runtime.agents.map(agent => <option key={agent.path} value={agent.path}>{agent.path}</option>)}</select></label>
      <label>受众<input aria-label="绘图受众" className="mt-1 w-full rounded border bg-background p-1" value={audience} onChange={event => setAudience(event.target.value)} placeholder="如：免疫学研究者" /></label>
      <label>绘图或修改需求<textarea aria-label="绘图需求" className="mt-1 min-h-32 w-full rounded border bg-background p-2" value={request} onChange={event => setRequest(event.target.value)} placeholder="希望表达什么？选中的部分需要如何修改？" /></label>
      <p className="text-xs text-muted-foreground">将先保存画布，再把需求和选中对象放入 Agent 草稿，供你检查并发送。</p>
      <Button type="submit" disabled={busy || !target || !request.trim()}>{busy ? "准备中…" : "交给 Agent"}</Button>
      {error && <p role="alert" className="text-destructive">{error}</p>}
    </form>
    <CanvasAnnotations projectId={projectId} canvasId={canvasId} getApi={getApi} getRevision={getRevision} ensureSaved={ensureSaved} />
  </aside>;
}
