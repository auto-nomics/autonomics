import { useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { CanvasPathAgent } from "./canvas-path-agent";

type Element = { id: string; type: string; isDeleted?: boolean; fileId?: string; customData?: { visualAssetElementId?: unknown; semanticAnchorId?: unknown } };
type Candidate = { id: string; base_revision: number; document_sha256?: string; document: unknown; review_markups?: { id: string; note: string; status: string }[] };
type Parts = { sha256: string; proposal_id: string; document_sha256: string; revision: number; annotatedDataURL: string; parts: { part_id: string; index: number }[] };
export function CanvasCandidatePath({ projectId, canvasId, candidate, focusedId, busy, onBusy, isDirty, onPrepared }: {
  projectId: string; canvasId: string; candidate: Candidate; focusedId?: string; busy: boolean; onBusy: (value: boolean) => void; isDirty: () => boolean; onPrepared: () => void;
}) {
  const [parts, setParts] = useState<Parts | null>(null), [selected, setSelected] = useState(0), [error, setError] = useState("");
  const [markupIds, setMarkupIds] = useState<string[]>([]);
  const doc = candidate.document as { elements?: Element[]; files?: Record<string, { mimeType?: string }> };
  const focused = doc.elements?.filter(e => e.id === focusedId && !e.isDeleted);
  const focus = focused?.length === 1 ? focused[0] : undefined;
  const image = focus?.type === "image" ? focus : doc.elements?.find(e => !e.isDeleted && e.type === "image" && e.id === focus?.customData?.visualAssetElementId && e.customData?.semanticAnchorId === focus?.id);
  if (!image?.fileId || doc.files?.[image.fileId]?.mimeType !== "image/svg+xml" || !candidate.document_sha256) return null;
  const hash = candidate.document_sha256;
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const query = `element_id=${encodeURIComponent(image.id)}&expected_revision=${candidate.base_revision}&proposal_id=${encodeURIComponent(candidate.id)}&document_sha256=${encodeURIComponent(hash)}`;
  const unchanged = () => !isDirty();
  return <details aria-label="候选素材路径修改" className="shrink-0 border-t text-xs"><summary className="cursor-pointer p-2">选择候选内部路径交给 Agent</summary><div className="max-h-64 space-y-2 overflow-auto p-2">
    <p>当前读取待审候选中的 SVG，不采用候选。路径编号只属于此文件快照。</p>
    <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
      onBusy(true); setError("");
      try {
        if (!unchanged()) throw new Error("请先保存或重新加载正式画布。");
        const value = await gatewayRequest("GET", `${path}/vector-parts?${query}`) as Parts;
        if (!unchanged() || value.proposal_id !== candidate.id || value.document_sha256 !== hash || value.revision !== candidate.base_revision) throw new Error("候选版本已变化，请重新打开。");
        setParts(value); setSelected(0);
      } catch (cause) { setError(String(cause)); } finally { onBusy(false); }
    }}>读取候选内部路径</Button>
    {error && <p role="alert" className="text-destructive">{error}</p>}
    {parts?.parts[selected] && <>
      <img alt="候选素材路径编号" src={parts.annotatedDataURL} className="max-h-36 w-full rounded bg-white object-contain" />
      <label>候选路径<select aria-label="候选素材路径" value={selected} disabled={busy} onChange={event => setSelected(Number(event.target.value))} className="ml-2 rounded border bg-background p-1">{parts.parts.map((part, index) => <option key={part.part_id} value={index}>{part.index}</option>)}</select></label>
      {candidate.review_markups?.some(m => m.status === "open") && <fieldset disabled={busy} className="space-y-1"><legend>本次路径修改关联的意见（不勾选将不关联到子候选）</legend>{candidate.review_markups.filter(m => m.status === "open").map(m => <label key={m.id} className="block"><input type="checkbox" checked={markupIds.includes(m.id)} onChange={event => setMarkupIds(ids => event.target.checked ? [...ids, m.id] : ids.filter(id => id !== m.id))} />{m.note}</label>)}</fieldset>}
      <CanvasPathAgent key={parts.parts[selected].part_id} projectId={projectId} canvasId={canvasId} elementId={image.id} revision={candidate.base_revision} sha256={parts.sha256} partId={parts.parts[selected].part_id} partIndex={parts.parts[selected].index} candidate={{ id: candidate.id, document_sha256: hash, markup_ids: markupIds }} busy={busy} onBusy={onBusy} onError={setError} unchanged={unchanged} onPrepared={onPrepared} capturePreview={async () => {
        await gatewayRequest("POST", `${path}/proposals/${encodeURIComponent(candidate.id)}/preview-request`, { expected_revision: candidate.base_revision });
        const deadline = Date.now() + 10000;
        while (Date.now() < deadline) {
          const preview = await gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}/preview?expected_revision=${candidate.base_revision}`) as { render_status: string; document_sha256: string };
          if (preview.document_sha256 !== hash || !unchanged()) throw new Error("候选预览版本已变化，请重新核对。");
          if (preview.render_status === "ready") return true;
          if (preview.render_status === "failed") throw new Error("候选预览失败，请重新核对。");
          await new Promise(resolve => setTimeout(resolve, 150));
        }
        return false;
      }} />
    </>}
  </div></details>;
}
