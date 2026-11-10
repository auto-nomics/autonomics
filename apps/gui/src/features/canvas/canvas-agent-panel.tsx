import { useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import type { Planning } from "./canvas-planning";
import type { MarkupPage } from "./canvas-markup-render";
import { canvasSelectionContext } from "./canvas-selection-context";
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
        const selection = canvasSelectionContext(api.getSceneElements(), selected);
        const objects = selection.objects;
        const annotations = (await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/annotations`) as Annotation[]).filter(note => note.status === "open").slice(0, 30);
        const reviewPage = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/markups?page=true&status=open`) as MarkupPage;
        const markups = reviewPage.items;
        const savedPlan = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/planning`) as Planning | null;
        const planning = savedPlan ? { version: savedPlan.version, based_on_revision: savedPlan.based_on_revision, status: savedPlan.status, issues: savedPlan.issues, proposal_id: savedPlan.proposal_id } : null;
        const manifest = await createSourceManifest({ scene: observed, title: "", project_id: projectId, figure_id: canvasId, based_on_revision: revision, unsaved_changes: false });
        const assetSources = { total: manifest.assets.length, truncated: manifest.assets.length > 30, assets: [...manifest.assets].sort((a,b) => Number(Boolean(selected[b.element_id ?? ""] || selected[b.semantic_anchor_id ?? ""])) - Number(Boolean(selected[a.element_id ?? ""] || selected[a.semantic_anchor_id ?? ""]))).slice(0, 30).map(asset => ({ element_id: asset.element_id, semantic_anchor_id: asset.semantic_anchor_id, mime_type: asset.mime_type, provider: asset.provider, name: asset.name, authors: asset.authors, licenses: asset.licenses, license_urls: asset.license_urls, source_urls: asset.source_urls, actual_sha256: asset.actual_sha256, hash_matches_recorded: asset.hash_matches_recorded, modifications: asset.modifications.slice(-3), missing_fields: asset.missing_fields })) };
        if (isDirty() || getRevision() !== revision || serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") !== observed) throw new Error("交接期间画布发生变化，请保存后重试。");
        const prompt = [
          "请作为科研绘图助手处理下面的画布需求。先用 visualization read 获取当前文档与版本；若 preview_available 为 true，使用 read_preview 查看同版本整图，选中参考图可用 read_image 查看；优先通过 propose 提交可在画布中预览的局部修改提案，保留无关元素。selected_objects 的 font_family/font_size/line_height、container_id 和排版字段反映当前选中文字，空值表示未知，不能凭空补齐；仍先 read 核对文档。修改默认科学字体的长标签时，优先使用 set_wrapped_text 同步原文、换行和尺寸；形状空间不足需先明确调整容器或字号；set_wrapped_text 可显式传入 font_size（4..200）在同一操作中改字号并重新换行，保留字体与行高，不自动扩大容器，随后核对候选是否重叠。只修改现有关系的方向或抑制端标记时，可在 propose/revise_proposal 中使用 set_relation_direction，明确给出 id、direction 和 inhibitory；该操作保留几何、文字和依据，不改写历史计划，仍须核对新方向与科学依据是否一致。整理已有组件的对齐和间距可使用 arrange，传入完整明确的 element_ids 和 mode，生成提案后用 read_proposal_preview 和返回的 proposal_id 检查候选图，核对基础版本与当前版本、重叠和连线；该预览不代表提案已采用，渲染尚未就绪可保持工作台打开并登录后重试；若需继续修正候选，先 read_proposal 获取候选文档与 document_sha256，再以原 proposal_id、该哈希及当前 expected_revision 调用 revise_proposal（operations 或 arrangement 二选一），保留累计修改；不要从正式文档重做而丢弃上一轮编辑。父候选关联开放标记时须显式给出本轮实际回应的 markup_ids（不继承则 []），生成子提案后再次 read_proposal_preview 核对。不要将区域注释当作组件参与分布。selected_objects 的 start_arrowhead/end_arrowhead 是当前端标记，declared_direction 是计划声明，手工改图后可能不同，须核对实际连线与科学意图。selected_objects 包含图片旋转/镜像/裁剪与已核验的放大图角色；inset_region 和 inset_callout 是区域注释，不是生物组件或科学关系。unverified_inset_reference 表示归属不明确，须 read 核对，不能直接据此删除。放大图是固定快照；检查更新先用 inset_status，再用 refresh_inset 提交提案，核对比例区域的科学含义。删除整套放大图只 remove 其 anchor_id 一次，不要重复删除级联成员。selected_objects_truncated 为 true 时不能把摘要当作完整选区；需要处理整个选区时，请让用户缩小选区或明确完整目标，read 文档本身不能恢复未提供的选中状态。markups 为待处理标记；若 markups_pagination.next_before 非空，使用 list_markups（status=open、before=该游标）继续查找更早意见，不能把当前页当作完整列表。若 markups 非空，使用 read_markup 和对应 markup_id 读取带标记的审阅图与说明；划除、补画、强调是审阅意图，不是正式元素。核对 at_revision，历史标记须与当前图稿比对后再修改。通过 propose 修改时，把实际回应的标记 ID 放入 markup_ids，供用户核对并在采用时选择是否完成这些意见；不得关联尚未处理的意见。需要素材时可使用 bioart（PNG）或 bioicons（SVG）；素材内部局部换色先用 bioicons parts 查看路径编号图，局部形变用 geometry 读取端点与控制点编号，再用 edit_parts 的 points 修改指定点，不自动移动相邻点；不要把控制点编号当成科学组件身份。translate 使用 SVG 根坐标，不是画布像素，需核对平移后上层裁剪和素材视口。不要用全素材同色替换代替局部修改。先预览并核对逐素材作者、许可与科学含义，不要把通用图标当作特定分子的证据。asset_sources 是同版本素材来源摘要；保留逐素材作者、许可及修改记录，缺失信息不得猜测补写。需要规划新图时使用 read_plan/save_plan，save_plan 优先通过有明确对象字段的 planning 参数提交计划（不要混入用于场景导入的 document）；nodes、edges、questions、assumptions 必须为 JSON 数组，待回答问题写入 questions 并保留空 answer，不要只藏在 message 说明文字中。保存图意、受众、组件、关系、假设和待澄清问题，向用户提问并保留已填写的答案；不要猜测缺失的科学机制。已有计划时用 read_plan 查看完整内容，使用 planning_version 和当前 expected_revision 更新计划；待澄清项完成并与用户核对后使用 sketch_plan 生成提案，或提示用户打开绘图计划进行编辑和生成。仅在没有已保存计划且用户已给出完整结构时使用直接 sketch。完成后说明修改和需要用户核对的科学假设，并提示点击查看 Agent 提案进行预览和采用。",
          JSON.stringify({ project_id: projectId, figure_id: canvasId, observed_revision: revision, audience: audience.trim(), request: request.trim(), selected_objects: objects, selected_objects_total: selection.total, selected_objects_truncated: selection.truncated, annotations, markups, markups_pagination: { status: "open", next_before: reviewPage.next_before }, planning, asset_sources: assetSources, preview_available: previewAvailable }, null, 2),
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
