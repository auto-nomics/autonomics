import { useCanvasIllustratorPath } from "./canvas-illustrator-context";
import { paintedCandidateFeedback } from "./canvas-illustration-overlaps";
import { appendCanvasRequestDraft } from "./canvas-request-draft";
import { useCandidateDraft } from "./canvas-candidate-drafts";
import { candidateFeedbackScope, type CandidateFeedbackComparison } from "./canvas-feedback-scope";
import { documentAttributeFeedback } from "./canvas-proposal-diff";
import type { CaptionPathReview } from "./canvas-caption-path-review";
import { illustrationConnectorLabelOverlaps } from "./canvas-connector-label-overlaps";
import { illustrationLabelOverlaps } from "./canvas-illustration-overlaps";
import { canvasSelectionContext,canvasMaterializationProgress,materializationContinuationRequest } from "./canvas-selection-context";
import { textOverlapChanges, textOverlaps } from "./canvas-text-overlaps";
import { useRef, useState } from "react";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { ObjectChange } from "./canvas-proposal-diff";
type Candidate = { document?: {elements?:readonly unknown[]|null}; id: string; summary?: string; base_revision: number; document_sha256?: string; review_markups?: { id: string; note: string; status: string }[] };
export function CanvasProposalFeedback({ projectId, canvasId, candidate, focusedId, changes, comparison, isDirty, onPrepared, disabled = false, onBusyChange }: {
  projectId: string; canvasId: string; candidate: Candidate; focusedId?: string; changes: ObjectChange[];
  comparison: CandidateFeedbackComparison;
  isDirty: () => boolean; onPrepared: () => void; disabled?: boolean; onBusyChange?: (value: boolean) => void;
}) {
  const runtime = useWorkbenchRuntime();
  const target = useCanvasIllustratorPath();
  const progress=canvasMaterializationProgress(candidate.document?.elements??[]),continuation=materializationContinuationRequest(progress);
  const [draft, setDraft, resetDraft] = useCandidateDraft(candidate, candidateFeedbackScope(focusedId, comparison), { request: "", markupIds: [] as string[] });
  const { request, markupIds } = draft;
  const targetAvailable = runtime.agents.some(agent => agent.path === target);
  const agentPaths = useRef(runtime.agents.map(agent => agent.path)); agentPaths.current = runtime.agents.map(agent => agent.path);
  const invalidMarkups = markupIds.filter(id => !candidate.review_markups?.some(markup => markup.id === id && markup.status === "open"));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <details className="shrink-0 border-t text-xs" aria-label="候选修订反馈"><summary className="cursor-pointer p-2">让 Agent 继续修订候选</summary><form className="flex flex-col gap-2 p-2" onSubmit={async event => {
    event.preventDefault(); if (busy || disabled || !targetAvailable || !request.trim() || invalidMarkups.length) return;
    setBusy(true); onBusyChange?.(true); setError("");
    try {
      if (isDirty()) throw new Error("请先保存或重新加载正式画布，再重新核对候选。");
      const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
      const [head, latest, geometry, paintedReceipt] = await Promise.all([
        gatewayRequest("GET", path) as Promise<{ metadata: { revision: number } }>,
        gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}`) as Promise<Candidate & { status: string; document: unknown }>,
        gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}/geometry-review`, undefined, { signal: AbortSignal.timeout(5000) }).catch(() => null) as Promise<{ proposal_id: string; document_sha256: string; scene_geometry_review: { caption_path_overlap?: CaptionPathReview } } | null>,
        candidate.document_sha256 ? gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}/image-bounds?expected_revision=${candidate.base_revision}&document_sha256=${encodeURIComponent(candidate.document_sha256)}`, undefined, { signal: AbortSignal.timeout(15000) }).catch(() => null) : Promise.resolve(null),
      ]);
      if (isDirty() || !candidate.document_sha256 || latest.document_sha256 !== candidate.document_sha256 || latest.status !== "pending" || latest.base_revision !== candidate.base_revision || head.metadata.revision !== candidate.base_revision) throw new Error("候选或正式版本已变化，请重新打开并核对提案。");
      if (markupIds.some(id => !latest.review_markups?.some(markup => markup.id === id && markup.status === "open"))) throw new Error("关联意见已变化，请重新打开并核对提案。");
      const baseline = comparison.kind === "parent_candidate"
        ? await gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(comparison.proposal_id)}`) as Candidate & { document: unknown }
        : await gatewayRequest("GET", `${path}/revisions/${comparison.revision}`) as { document: unknown };
      if (comparison.kind === "parent_candidate" && (!comparison.document_sha256 || !("document_sha256" in baseline) || baseline.document_sha256 !== comparison.document_sha256)) throw new Error("对照候选已变化，请重新打开提案。");
      if (comparison.kind === "formal_revision" && comparison.revision !== candidate.base_revision) throw new Error("对照版本不匹配，请重新打开提案。");
      if (isDirty()) throw new Error("画布发生变化，请保存后重新核对候选。");
      const selectedContext = (document: unknown) => {
        const elements = document && typeof document === "object" && "elements" in document && Array.isArray(document.elements) ? document.elements : [];
        return canvasSelectionContext(elements, focusedId ? { [focusedId]: true } : {});
      };
      const readScope = { project_id: projectId, figure_id: canvasId, proposal_id: candidate.id, expected_revision: candidate.base_revision };
      const candidateElements = latest.document && typeof latest.document === "object" && "elements" in latest.document && Array.isArray(latest.document.elements) ? latest.document.elements : [];
      const focusedMatches = focusedId ? candidateElements.filter((element: unknown) => element && typeof element === "object" && "id" in element && element.id === focusedId) : [];
      const focusedElement = focusedMatches.length === 1 ? focusedMatches[0] : null;
      const scopedIds = focusedId && focusedId.trim() && new TextEncoder().encode(focusedId).length <= 512 && focusedElement && !("isDeleted" in focusedElement && focusedElement.isDeleted === true) ? { element_ids: [focusedId] } : {};
      const followUpInputs = { read_proposal: { action: "read_proposal", ...readScope, ...scopedIds }, read_proposal_preview: { action: "read_proposal_preview", ...readScope }, ...(scopedIds.element_ids && focusedElement && "type" in focusedElement && focusedElement.type === "image" ? { image_info: { action: "image_info", ...readScope, document_sha256: latest.document_sha256, element_id: focusedId } } : {}) };
      const captionPathCheck = geometry?.proposal_id === latest.id && geometry?.document_sha256 === latest.document_sha256 && geometry?.scene_geometry_review?.caption_path_overlap
        ? { available: true, proposal_id: latest.id, document_sha256: latest.document_sha256, ...geometry.scene_geometry_review.caption_path_overlap }
        : { available: false };
      const paintedCheck = await paintedCandidateFeedback(latest.document, paintedReceipt, latest.id, latest.document_sha256!);
      if (isDirty()) throw new Error("画布发生变化，请保存后重新核对候选。");
      const context = { materialization_checkpoint:canvasMaterializationProgress(candidateElements), reviewed_document_attributes: documentAttributeFeedback(baseline.document, latest.document), follow_up_inputs: followUpInputs, project_id: projectId, figure_id: canvasId, proposal_id: candidate.id, document_sha256: candidate.document_sha256, expected_revision: candidate.base_revision, prepared_request: request.trim(), candidate_caption_path_check: captionPathCheck, candidate_sampled_illustration_label_check: paintedCheck, candidate_illustration_label_check: illustrationLabelOverlaps(latest.document), candidate_connector_label_check: illustrationConnectorLabelOverlaps(latest.document), candidate_text_overlap_check: { scope: "candidate_document", method: "unrotated_text_rectangles", ...textOverlaps(latest.document), comparison_changes: textOverlapChanges(baseline.document,latest.document) }, focused_object_id: focusedId ?? null, focused_object_context: focusedId ? { candidate: selectedContext(latest.document), comparison: selectedContext(baseline.document) } : null, comparison, reviewed_object_change: focusedId ? changes.find(change => change.id === focusedId) ?? null : null, markup_ids: markupIds };
      const prompt = ["请继续修订下面的待审科研绘图候选。可原样复制 follow_up_inputs.read_proposal 作为 visualization 工具参数（已包含项目、画布、候选 ID 与图稿 expected_revision）。聚焦的存活对象会在 read_proposal 参数中携带 element_ids，返回 selected_elements 原始对象和有省略计数的 geometry_context；候选哈希与全图诊断仍对应完整候选，禁止将投影当完整场景覆盖。需求涉及其他对象时扩大 ID 列表，需要完整规划或整图内容时省略 element_ids 读取。已明确对象和原生操作的局部修订可直接使用所列工具，不必为同一操作重复查询技能目录或重读无关规划；用户明确指定技能或确有缺失能力时仍按需使用。局部排版任务保持当前已审阅文字和关系；不要仅因历史差异建议补回旧位点或表述，科学修订须另有授权与来源依据。完成后简要说明改动、待审候选和需核对之处，除非用户要求，不重复完整科学背景或逐工具过程。先用 visualization read_proposal 核对候选 ID、document_sha256、基础版本与当前版本，再 read_proposal_preview 检查候选图。聚焦图片时可原样复制 follow_up_inputs.image_info 读取该候选图片的自然尺寸、裁剪与文件哈希，不会返回图片字节；必须保留候选ID/文档哈希，省略候选引用只查正式图，找不到尚未采用的图片。文字、几何或整理修改使用 visualization revise_proposal，以该候选为基础提交 operations 或 arrangement。在候选上追加已明确结构的流程时，可在 operations 中使用 insert_sketch，传入 plan（message、audience、layout、nodes、edges、assumptions）、新的 id_prefix 和数值 x/y；保留已有对象和文件，一次生成绑定的原生节点、文字和箭头，不用整图 sketch 覆盖已有内容。新增区室/背景素材时独立 insert 并加可见限定，不用细胞器图标替换分子节点或伪设语义锚点。可先用 visualization_find_space，传当前候选ID/哈希、版本、参考对象relative_to和所需width/height/gap，查询原生外框避让建议；它不证明像素或科学适用性，需核对PNG。新增独立素材使用 bioart 或 bioicons insert，传入当前候选引用、明确的 markup_ids 和场景 x/y，创建累积子候选；insert 不传 element_id。移动或缩放普通矩形/椭圆/菱形节点时，使用 set_node_geometry，传 id 和数值 x/y/width/height；保持完整绑定文字与字号，更新居中尖角连线及其说明，其他节点与文件不变。移动或缩放独立图片使用 visualization revise_proposal 的 set_image_geometry，id/x/y/width/height 中几何字段使用 JSON 数字；scale 只表示 ±1 镜像，不表示缩放比例。已有节点的外观使用 replace，保留节点及关系身份。已绑定语义节点的插图需要调整位置和尺寸时，使用 set_illustration_geometry 操作，传入图片或锚点 id、当前 file_id、数值 x/y/width/height 及明确核对的 normalized_bounds；它保留像素与科学标签，重算语义锚点和连接端点。标签位置单独核对，不要使用仅支持独立图片的 set_image_geometry 绕过绑定。选区中的 illustration_fit 只描述贴合快照是否对应当前文件和视口；stale_file/stale_geometry 表示原记录已过期，不得沿用旧文件的 normalizedBounds 当新采样。用户要求按当前未裁剪未旋转 SVG 重新贴合时，可使用 visualization_fit_svg_preview，传精确候选ID/哈希、当前版本、element_id及当前file_id，后端会读取当前文件的采样范围并生成受保护子候选；须核对PNG及全图间距，不自动采用。素材替换先检索并预览同一素材、核对含义及来源许可，再使用 bioart 或 bioicons replace，传入下方 proposal_id、document_sha256、expected_revision 和明确的 markup_ids，在这份候选上创建子候选。候选 SVG 的检查使用 bioicons palette、parts 或 geometry，必须传入当前候选的 proposal_id、document_sha256 和 expected_revision，只读检查不传 markup_ids；配色或局部路径修改使用 recolor 或 edit_parts，并明确传入 markup_ids。edit_parts 的 sha256 和 part_id 必须来自这份候选最新的 parts/geometry，不能用文档哈希代替 SVG 哈希。每次生成子候选后更新候选 ID 与文档哈希，并重新检查素材，不能沿用父候选的路径 ID。插图范围贴合可使用 visualization_fit，必填 element_id、file_id 与归一化原始素材坐标数值 x/y/width/height（SVG 使用根坐标，PNG 使用原图范围；位图不能调用 SVG parts），候选作用域和哈希与 revise_proposal 相同；它创建子候选，须继续沿返回的新 ID/哈希修改。只移动一份未绑定文字或插图独立标签时，优先使用 visualization_move_text，传入下方当前候选作用域、document_sha256、文字 element_id 和顶层 JSON 数值 dx/dy；保留文字、换行、字号和分组归属，不扩展到图片、锚点或连接线。绑定容器的文字须调整容器或文字排版，不能用此入口。visualization_translate 会扩展分组并移动整个组件，不能用于仅移动一个分组标签。整体平移现有对象可使用 visualization_translate（顶层 element_ids、数值 dx/dy，候选作用域与哈希必填）或 translate 操作，保留对象 ID；不要用 duplicate 后 remove 代替移动。依赖前一步结果的修改必须串行等待工具返回，并用返回的新 proposal_id/document_sha256 继续；也可将兼容操作放在同一次 operations 中。对同一父候选并行提交会得到互不合并的兄弟分支，不能认为最新创建的候选包含其他分支的修改。保留上一轮累计修改和无关对象，不先采用父候选，不在正式图另起提案；通用插图不代表特异分子结构或新机制。focused_object_id 是用户当前核对的对象，可能尚未修改或仅存在于对照版本。focused_object_context 分别给出 candidate 和 comparison 中该 ID 的有界摘要；缺失对象不会从另一版本补入，摘要不是完整素材或编辑指令，需读取该候选的原始对象确认身份。reviewed_object_change 是当前对照视角的有界字段摘要，不是完整编辑指令。reviewed_document_attributes 对照 comparison 给出文档顶层属性变化，有 total/omitted 计数和 text_truncated 标记；它不是完整文档或修改指令，不授权更改来源或其他未要求的属性。candidate_illustration_label_check 检查标签与其明确所属插图的旋转显示外框。图片留白也可能相交，不等于实际字形碰撞；skipped/omitted 表示检查不完整。先核对 PNG，按用户要求调整位置或字号，不能删掉科学标签。差异的数量、成组出现或对照边未动都不能证明用户是否有意修改，不据此揣测用户意图。forward 仅表示起点到终点的图形方向；inhibitory=true 时终点是抑制横杠，不是普通箭头。双向端标记不独立证明可逆机制。用户要求简短审阅时，优先用三条简短说明交代差异、核对局限及只读结论，不重复全部技术ID、工具过程或其他无差异诊断。改善换行时保留完整分子名和 originalText，不得通过缩写或删词消除显示问题。candidate_caption_path_check 是后端对同一 ID/哈希候选中尖角原生箭头中心线与所属说明文字框的有界覆盖检查。available=false 或 skipped_count/warning_omitted_count 表示不可用或检查不完整；覆盖率不等于实际字形遮挡，只按用户要求核对 PNG、调整间距或走线，不删除或缩写科学说明来消除提示，不自动修复无关对象。candidate_sampled_illustration_label_check 是同一候选文件字节哈希核对后的SVG预览采样包围盒检查，原视口检查仍保留。available=false、complete=false、unsupported_images、skipped或omitted表示检查不可用或覆盖不完整；回退图片按完整视口检查。主viewport/sampled字段仅检查所属独立标签；all_image_text字段另对全部插图与文字（含其他标签和连线说明）做有界检查，也须核对其complete/skipped/omitted。不检查插图之间或连线路径的碰撞。采样范围包含不透明背景和装饰，不是精确矢量轮廓。仅视口相交而采样包围盒不相交时，不应单凭透明留白警告移动对象；仍须结合PNG与本轮要求判断，零提示不证明科学含义或全部排版正确。candidate_text_overlap_check 是对同一哈希候选的有界文字矩形检查；pairs 给出需核对的对象 ID，skipped/truncated 表示检查不完整。它可能包含原有重叠或文字空白，不代表实际字形碰撞，更不代表应删除文字；须通过候选预览确认，保留科学含义，再按用户要求修改。comparison_changes 按 comparison 来源区分 introduced/persisting/resolved/unknown；unknown 或 skipped 表示不能确定，不能当作新增或已修复。零条提示也不证明布局没有问题。markup_ids 是用户本轮选择回应的意见，[] 表示不继承关联，不得自动添加或完成其他意见。生成子提案后再次读取候选预览核对，并向用户说明需要确认的科学假设。缺失的科学事实应先询问，不要猜测。", JSON.stringify(context, null, 2)].join("\n\n");
      if (!agentPaths.current.includes(target)) throw new Error("绑定的插画专家已不可用，请重新连接；修订草稿已保留。");
      runtime.setDraftFor(target, previous => appendCanvasRequestDraft(previous, request, { title: `候选修订上下文 · ${latest.summary || "待审候选"}`, source: `visualization:${projectId}/${canvasId}/proposals/${candidate.id}`, revision: `${candidate.base_revision}:${candidate.document_sha256}`, prompt }));
      resetDraft(); runtime.selectAgent(target); onPrepared(); openAgentEditor(target);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); onBusyChange?.(false); }
  }}>
    {progress.total>0&&<div aria-label="候选素材化进度" className="space-y-1"><p>组件 {progress.total}：缺主图 {progress.counts.missing}，待贴合 {progress.counts.needs_fit}，归属需核对 {progress.counts.ambiguous}，贴合快照对应 {progress.counts.snapshot_current}{progress.truncated?'（列表有省略）':''}。</p><p>仅统计归属与贴合记录，不代表素材适用、来源或整图已审阅。</p>{progress.label_ownership_issues.total>0&&<p>有 {progress.label_ownership_issues.total} 个已插图组件未核实标签归属（{progress.label_ownership_issues.items.join("、")}）。文字可见不代表已加入组件组；请明确关联已有标签，避免移动时遗漏。</p>}{progress.unassigned_images.total>0&&<p>另有 {progress.unassigned_images.total} 张图片未核实组件归属；可能是独立背景或尚未绑定的素材，不计为组件完成。请核对用途，避免重复插入。</p>}{continuation&&<Button type="button" size="sm" variant="outline" disabled={busy||disabled} onClick={()=>setDraft(current=>({...current,request:current.request.trim()?`${current.request}\n\n${continuation}`:continuation}))}>追加分步续作要求</Button>}</div>}
    <label>本轮修改要求<textarea aria-label="候选修订要求" disabled={busy || disabled} className="mt-1 min-h-16 w-full rounded border bg-background p-2" value={request} onChange={event => setDraft(current => ({ ...current, request: event.target.value }))} maxLength={12000} /></label>
    <p>{focusedId ? `当前核对对象：${focusedId}` : "当前核对整份候选"}；需求将放入 Agent 草稿，供你检查并发送。</p>
    <p className="text-muted-foreground">修改要求按当前候选、对象与对照版本暂存；切换后可返回继续，刷新或关闭画布页后清除。</p>
    {target && !targetAvailable && <p role="status">绑定的插画专家不可用，请在画布菜单中重新连接。</p>}
    {invalidMarkups.length > 0 && <div role="status"><p>已选意见的状态发生变化，请重新核对。</p><Button type="button" size="sm" variant="outline" disabled={busy || disabled} onClick={() => setDraft(current => ({ ...current, markupIds: current.markupIds.filter(id => candidate.review_markups?.some(markup => markup.id === id && markup.status === "open")) }))}>移除已失效的意见选择</Button></div>}
    {(candidate.review_markups ?? []).filter(markup => markup.status === "open").map(markup => <label key={markup.id} className="flex items-start gap-2"><input type="checkbox" disabled={busy || disabled} checked={markupIds.includes(markup.id)} onChange={event => setDraft(current => ({ ...current, markupIds: event.target.checked ? [...current.markupIds, markup.id] : current.markupIds.filter(id => id !== markup.id) }))} />本轮回应：{markup.note}</label>)}
    <Button type="submit" size="sm" disabled={busy || disabled || !targetAvailable || !request.trim() || invalidMarkups.length > 0}>{busy ? "核对候选版本…" : "准备候选修订草稿"}</Button>
    <Button type="button" size="sm" variant="outline" disabled={busy || disabled} onClick={() => { resetDraft(); setError(""); }}>清除此处修订草稿</Button>
    {error && <p role="alert" className="text-destructive">{error}</p>}
  </form></details>;
}
