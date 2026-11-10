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
        gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}`) as Promise<Candidate & { status: string }>,
      ]);
      if (isDirty() || !candidate.document_sha256 || latest.document_sha256 !== candidate.document_sha256 || latest.status !== "pending" || latest.base_revision !== candidate.base_revision || head.metadata.revision !== candidate.base_revision) throw new Error("候选或正式版本已变化，请重新打开并核对提案。");
      if (markupIds.some(id => !latest.review_markups?.some(markup => markup.id === id && markup.status === "open"))) throw new Error("关联意见已变化，请重新打开并核对提案。");
      const context = { project_id: projectId, figure_id: canvasId, proposal_id: candidate.id, document_sha256: candidate.document_sha256, expected_revision: candidate.base_revision, request: request.trim(), focused_object_id: focusedId ?? null, comparison, reviewed_object_change: focusedId ? changes.find(change => change.id === focusedId) ?? null : null, markup_ids: markupIds };
      const prompt = ["请继续修订下面的待审科研绘图候选。先用 visualization read_proposal 核对候选 ID、document_sha256、基础版本与当前版本，再 read_proposal_preview 检查候选图。使用 revise_proposal 以该候选为基础提交 operations 或 arrangement，保留上一轮累计修改和无关对象，不采用提案。focused_object_id 是用户当前核对的对象，可能仅存在于对照版本；需在完整候选中确认身份。reviewed_object_change 是当前对照视角的有界字段摘要，不是完整编辑指令。markup_ids 是用户本轮选择回应的意见，[] 表示不继承关联，不得自动添加或完成其他意见。生成子提案后再次读取候选预览核对，并向用户说明需要确认的科学假设。缺失的科学事实应先询问，不要猜测。", JSON.stringify(context, null, 2)].join("\n\n");
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
