import { appendCanvasRequestDraft } from "./canvas-request-draft";
import { svgPathReadInputs } from "./canvas-svg-path-context";
import { useSvgPathInspection } from "./canvas-svg-path-channel";
import { useRef, useState } from "react";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";

type PathInspection = {
  revision: number; proposal_id?: string; document_sha256?: string; sha256: string; file_id: string; element_id: string;
  render_contribution?: unknown;
  parts: { part_id: string; index: number; bounds: number[]; fill: string; stroke: string; stroke_width: number; opacity: number; changed_preview_pixels_when_hidden?: number }[];
};
export function CanvasPathAgent({ projectId, canvasId, elementId, revision, sha256, partId, partIndex, busy, unchanged, onBusy, onError, onPrepared, capturePreview, candidate }: {
  candidate?: { id: string; document_sha256: string; markup_ids: string[] };
  capturePreview: () => Promise<boolean>; projectId: string; canvasId: string; elementId: string; revision: number; sha256: string; partId: string; partIndex: number;
  busy: boolean; unchanged: () => boolean; onBusy: (busy: boolean) => void; onError: (error: string) => void; onPrepared: () => void;
}) {
  useSvgPathInspection({project_id:projectId,figure_id:canvasId,element_id:elementId,expected_revision:revision,sha256,part_id:partId,part_index:partIndex,...(candidate?{proposal_id:candidate.id,document_sha256:candidate.document_sha256,markup_ids:candidate.markup_ids}:{})});
  const runtime = useWorkbenchRuntime();
  const runtimeRef = useRef(runtime); runtimeRef.current = runtime;
  const [target, setTarget] = useState(runtime.activeAgent || runtime.agents[0]?.path || "");
  const [request, setRequest] = useState("");
  const targetAvailable = runtime.agents.some(agent => agent.path === target);
  return <section data-svg-part-id={partId} aria-label="交给 Agent 修改路径" className="mt-3 space-y-2 border-t pt-3 text-sm">
    <p>将当前路径和修改要求放入 Agent 草稿，检查后再发送。未预览的手动修改不会自动传入。</p>
    <label className="block">Agent<select aria-label="路径修改 Agent" disabled={busy} value={target} className="ml-2 rounded border bg-background p-1" onChange={event => setTarget(event.target.value)}><option value="">选择 Agent</option>{runtime.agents.map(agent => <option key={agent.path} value={agent.path}>{agent.path}</option>)}</select></label>
    <textarea aria-label="路径修改要求" disabled={busy} value={request} className="min-h-20 w-full rounded border bg-background p-2" placeholder="例如：将这条路径改为蓝色，保留其他部分" onChange={event => setRequest(event.target.value)} />
    <Button size="sm" disabled={busy || !targetAvailable || !request.trim()} onClick={async () => {
      if (busy || !targetAvailable || !request.trim()) return;
      onBusy(true); onError("");
      try {
        if (!unchanged()) throw new Error("画布已变化，请重新读取路径后再准备草稿。");
        if (!runtime.agents.some(agent => agent.path === target)) throw new Error("Agent 已不可用，请重新选择。");
        const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
        const query = candidate ? `&proposal_id=${encodeURIComponent(candidate.id)}&document_sha256=${encodeURIComponent(candidate.document_sha256)}` : "";
        const latest = await gatewayRequest("GET", `${path}/vector-parts?element_id=${encodeURIComponent(elementId)}&expected_revision=${revision}${query}`) as PathInspection;
        const part = latest.parts.find(part => part.part_id === partId);
        if (!unchanged() || latest.revision !== revision || latest.sha256 !== sha256 || latest.element_id !== elementId || !part) throw new Error("素材版本或路径已变化，请关闭面板并重新读取。");
        if (candidate) {
          if (latest.proposal_id !== candidate.id || latest.document_sha256 !== candidate.document_sha256) throw new Error("候选身份已变化，请重新读取。");
          const current = await gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}`) as { status: string; base_revision: number; document_sha256: string; review_markups?: { id: string; status: string }[] };
          if (current.status !== "pending" || current.base_revision !== revision || current.document_sha256 !== candidate.document_sha256 || candidate.markup_ids.some(id => !current.review_markups?.some(m => m.id === id && m.status === "open"))) throw new Error("候选或关联意见已变化，请重新核对。");
        }
        const previewAvailable = await capturePreview();
        if (!unchanged()) throw new Error("生成预览期间画布已变化，请重新读取路径。");
        const scope = { project_id: projectId, figure_id: canvasId, expected_revision: revision, element_id: elementId, ...(candidate ? { proposal_id: candidate.id, document_sha256: candidate.document_sha256 } : {}) };
        const context = {
          prepared_request: request.trim(), preview_available: previewAvailable, scope: candidate ? "pending_candidate_svg_path" : "saved_formal_svg_path", ...(candidate ? { markup_ids: candidate.markup_ids } : {}), ...scope, file_id: latest.file_id, sha256,
          selected_path: { part_id: part.part_id, index: part.index, bounds: part.bounds, fill: part.fill, stroke: part.stroke, stroke_width: part.stroke_width, opacity: part.opacity, changed_preview_pixels_when_hidden: part.changed_preview_pixels_when_hidden },
          render_contribution: latest.render_contribution,
          follow_up_inputs: svgPathReadInputs({project_id:projectId,figure_id:canvasId,element_id:elementId,expected_revision:revision,sha256,part_id:partId,part_index:partIndex,...(candidate?{proposal_id:candidate.id,document_sha256:candidate.document_sha256}:{})},previewAvailable),
        };
        const candidateGuide = candidate ? "这是未采用候选，先复制 read_proposal/read_proposal_preview 核对候选；parts/geometry/edit_parts 必须带 proposal_id/document_sha256/expected_revision。沿此候选生成子候选，保留前序修改与父候选，不在正式图另起根提案，不自动采用。edit_parts 显式传下面的 markup_ids；它是用户选择本次关联的意见，空数组明确不关联。" : "";
        const prompt = [candidateGuide, "请按下面的请求修改已保存科研图中的指定 SVG 图形路径。先用 visualization 的 follow_up_inputs.read 或 read_proposal 核对版本及图片，再用 bioicons 的 parts（需要点编辑时用 geometry）读取当前快照与编号图；follow_up_inputs.read_preview 或 read_proposal_preview 读取同版本整图预览，需核对 preview_available；正式图的 read_image 读取此素材图片。sha256/part_id 只属于该文件快照，版本不符须停止并重新核对，不能猜编号。read 的局部投影不是完整文档，禁止将其整体覆盖图稿。仅按本条消息正文修改，prepared_request只记录准备时的旧要求，未指定的其他路径、其他对象、来源许可与图像几何保持。路径编号不是生物学身份；渲染贡献不是面积或可见性证明，须查看修改后的 PNG。用 bioicons edit_parts 创建待审提案，不直接提交正式图，不自动采用；每次修改后刷新 parts 和快照编号。下方只描述已保存状态，不包含面板里尚未预览采用的手动修改。", JSON.stringify(context, null, 2)].join("\n\n");
        if (!runtimeRef.current.agents.some(agent => agent.path === target)) throw new Error("Agent 已不可用，请重新选择；要求已保留。");
        runtime.setDraftFor(target, previous => appendCanvasRequestDraft(previous, request, { title: `SVG 路径上下文 · 路径 ${part.index}`, source: `visualization:${projectId}/${canvasId}${candidate ? `/proposals/${candidate.id}` : ""}/images/${elementId}/parts/${partId}`, revision: `${revision}:${sha256}${candidate ? `:${candidate.document_sha256}` : ""}`, prompt }));
        runtime.selectAgent(target); openAgentEditor(target); onPrepared();
      } catch (cause) { onError(String(cause)); } finally { onBusy(false); }
    }}>准备路径修改草稿</Button>
  </section>;
}
