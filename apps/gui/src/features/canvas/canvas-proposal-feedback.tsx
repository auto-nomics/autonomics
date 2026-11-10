import { canvasSelectionContext } from "./canvas-selection-context";
import { textOverlapChanges, textOverlaps } from "./canvas-text-overlaps";
import { useState } from "react";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { ObjectChange } from "./canvas-proposal-diff";
type Candidate = { id: string; base_revision: number; document_sha256?: string; review_markups?: { id: string; note: string; status: string }[] };
export function CanvasProposalFeedback({ projectId, canvasId, candidate, focusedId, changes, comparison, isDirty, onPrepared }: {
  projectId: string; canvasId: string; candidate: Candidate; focusedId?: string; changes: ObjectChange[];
  comparison: { kind: "formal_revision"; revision: number } | { kind: "parent_candidate"; proposal_id: string; document_sha256?: string };
  isDirty: () => boolean; onPrepared: () => void;
}) {
  const runtime = useWorkbenchRuntime();
  const [target, setTarget] = useState(runtime.activeAgent || runtime.agents[0]?.path || "");
  const [request, setRequest] = useState("");
  const [markupIds, setMarkupIds] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <details className="shrink-0 border-t text-xs" aria-label="候选修订反馈"><summary className="cursor-pointer p-2">让 Agent 继续修订候选</summary><form className="flex max-h-60 flex-col gap-2 overflow-auto p-2" onSubmit={async event => {
    event.preventDefault(); if (busy || !target || !request.trim()) return;
    setBusy(true); setError("");
    try {
      if (isDirty()) throw new Error("请先保存或重新加载正式画布，再重新核对候选。");
      const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
      const [head, latest] = await Promise.all([
        gatewayRequest("GET", path) as Promise<{ metadata: { revision: number } }>,
        gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}`) as Promise<Candidate & { status: string; document: unknown }>,
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
      const followUpInputs = { read_proposal: { action: "read_proposal", ...readScope }, read_proposal_preview: { action: "read_proposal_preview", ...readScope } };
      const context = { follow_up_inputs: followUpInputs, project_id: projectId, figure_id: canvasId, proposal_id: candidate.id, document_sha256: candidate.document_sha256, expected_revision: candidate.base_revision, request: request.trim(), candidate_text_overlap_check: { scope: "candidate_document", method: "unrotated_text_rectangles", ...textOverlaps(latest.document), comparison_changes: textOverlapChanges(baseline.document,latest.document) }, focused_object_id: focusedId ?? null, focused_object_context: focusedId ? { candidate: selectedContext(latest.document), comparison: selectedContext(baseline.document) } : null, comparison, reviewed_object_change: focusedId ? changes.find(change => change.id === focusedId) ?? null : null, markup_ids: markupIds };
      const prompt = ["请继续修订下面的待审科研绘图候选。可原样复制 follow_up_inputs.read_proposal 作为 visualization 工具参数（已包含项目、画布、候选 ID 与图稿 expected_revision）。先用 visualization read_proposal 核对候选 ID、document_sha256、基础版本与当前版本，再 read_proposal_preview 检查候选图。文字、几何或整理修改使用 visualization revise_proposal，以该候选为基础提交 operations 或 arrangement。新增独立素材使用 bioart 或 bioicons insert，传入当前候选引用、明确的 markup_ids 和场景 x/y，创建累积子候选；insert 不传 element_id。移动或缩放独立图片使用 visualization revise_proposal 的 set_image_geometry，id/x/y/width/height 中几何字段使用 JSON 数字；scale 只表示 ±1 镜像，不表示缩放比例。已有节点的外观使用 replace，保留节点及关系身份。素材替换先检索并预览同一素材、核对含义及来源许可，再使用 bioart 或 bioicons replace，传入下方 proposal_id、document_sha256、expected_revision 和明确的 markup_ids，在这份候选上创建子候选。候选 SVG 的检查使用 bioicons palette、parts 或 geometry，必须传入当前候选的 proposal_id、document_sha256 和 expected_revision，只读检查不传 markup_ids；配色或局部路径修改使用 recolor 或 edit_parts，并明确传入 markup_ids。edit_parts 的 sha256 和 part_id 必须来自这份候选最新的 parts/geometry，不能用文档哈希代替 SVG 哈希。每次生成子候选后更新候选 ID 与文档哈希，并重新检查素材，不能沿用父候选的路径 ID。整体平移现有对象使用 translate（element_ids、数值 dx/dy），保留对象 ID；不要用 duplicate 后 remove 代替移动。保留上一轮累计修改和无关对象，不先采用父候选，不在正式图另起提案；通用插图不代表特异分子结构或新机制。focused_object_id 是用户当前核对的对象，可能尚未修改或仅存在于对照版本。focused_object_context 分别给出 candidate 和 comparison 中该 ID 的有界摘要；缺失对象不会从另一版本补入，摘要不是完整素材或编辑指令，需在完整候选中确认身份。reviewed_object_change 是当前对照视角的有界字段摘要，不是完整编辑指令。candidate_text_overlap_check 是对同一哈希候选的有界文字矩形检查；pairs 给出需核对的对象 ID，skipped/truncated 表示检查不完整。它可能包含原有重叠或文字空白，不代表实际字形碰撞，更不代表应删除文字；须通过候选预览确认，保留科学含义，再按用户要求修改。comparison_changes 按 comparison 来源区分 introduced/persisting/resolved/unknown；unknown 或 skipped 表示不能确定，不能当作新增或已修复。零条提示也不证明布局没有问题。markup_ids 是用户本轮选择回应的意见，[] 表示不继承关联，不得自动添加或完成其他意见。生成子提案后再次读取候选预览核对，并向用户说明需要确认的科学假设。缺失的科学事实应先询问，不要猜测。", JSON.stringify(context, null, 2)].join("\n\n");
      const previous = runtime.drafts[target] || "";
      runtime.setDraftFor(target, previous ? `${previous}\n\n${prompt}` : prompt);
      runtime.selectAgent(target); onPrepared(); openAgentEditor(target);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>
    <label>Agent<select aria-label="候选修订 Agent" className="ml-2 rounded border bg-background p-1" value={target} onChange={event => setTarget(event.target.value)}><option value="">选择 Agent</option>{runtime.agents.map(agent => <option key={agent.path} value={agent.path}>{agent.path}</option>)}</select></label>
    <label>本轮修改要求<textarea aria-label="候选修订要求" className="mt-1 min-h-16 w-full rounded border bg-background p-2" value={request} onChange={event => setRequest(event.target.value)} maxLength={12000} /></label>
    <p>{focusedId ? `当前核对对象：${focusedId}` : "当前核对整份候选"}；需求将放入 Agent 草稿，供你检查并发送。</p>
    {(candidate.review_markups ?? []).filter(markup => markup.status === "open").map(markup => <label key={markup.id} className="flex items-start gap-2"><input type="checkbox" checked={markupIds.includes(markup.id)} onChange={event => setMarkupIds(current => event.target.checked ? [...current, markup.id] : current.filter(id => id !== markup.id))} />本轮回应：{markup.note}</label>)}
    <Button type="submit" size="sm" disabled={busy || !target || !request.trim()}>{busy ? "核对候选版本…" : "准备候选修订草稿"}</Button>
    {error && <p role="alert" className="text-destructive">{error}</p>}
  </form></details>;
}
