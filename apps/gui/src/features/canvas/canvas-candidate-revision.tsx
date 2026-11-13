import { useCandidateDraft } from "./canvas-candidate-drafts";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
export type EditableCandidate = { id: string; base_revision: number; document_sha256?: string; document: ExcalidrawInitialDataState; review_markups?: { id: string; note: string; status: string }[] };
export type CandidateRevisionProps = { projectId: string; canvasId: string; candidate: EditableCandidate; busy: boolean; onBusy: (value: boolean) => void; isDirty: () => boolean; onRevised: (id: string) => Promise<void> };
export function CanvasCandidateRevision({ projectId, canvasId, candidate, busy, onBusy, isDirty, onRevised, label, title, summary, submitLabel, operations, arrangement, valid, children, draftKey, onCommitted }: CandidateRevisionProps & {
  label: string; title: string; summary: string; submitLabel: string; valid: boolean; children: ReactNode; draftKey: readonly string[]; onCommitted: () => void;
} & ({operations: Record<string, unknown>[]; arrangement?: never} | {operations?: never; arrangement: {element_ids:string[];mode:string}})) {
  const [error, setError] = useState("");
  const [markupIds, setMarkupIds, resetMarkups] = useCandidateDraft<string[]>(candidate, [...draftKey, "markups"], []);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  return <details data-candidate-id={candidate.id} aria-label={label} className="shrink-0 border-t text-xs"><summary className="cursor-pointer p-2">{title}</summary><form className="space-y-2 p-2" onSubmit={async event => {
    event.preventDefault(); if (busy || !valid || !candidate.document_sha256) return;
    onBusy(true); setError("");
    try {
      if (isDirty()) throw new Error("请先保存或重新加载正式画布，再核对候选。");
      const [head, latest] = await Promise.all([
        gatewayRequest("GET", path) as Promise<{ metadata: { revision: number } }>,
        gatewayRequest("GET", `${path}/proposals/${encodeURIComponent(candidate.id)}`) as Promise<EditableCandidate & { status: string }>,
      ]);
      if (!active.current) return;
      if (isDirty() || head.metadata.revision !== candidate.base_revision || latest.id !== candidate.id || latest.status !== "pending" || latest.base_revision !== candidate.base_revision || latest.document_sha256 !== candidate.document_sha256) throw new Error("候选或正式版本已变化，请重新打开核对。");
      if (markupIds.some(id => !latest.review_markups?.some(markup => markup.id === id && markup.status === "open"))) throw new Error("关联意见已变化，请重新打开核对。");
      const child = await gatewayRequest("POST", `${path}/proposals/${encodeURIComponent(candidate.id)}/revisions`, {
        expected_revision: candidate.base_revision, parent_document_sha256: candidate.document_sha256, summary, markup_ids: markupIds, ...(arrangement ? {arrangement} : {operations}),
      }) as { id: string };
      if (!active.current) return;
      if (isDirty()) throw new Error("生成期间正式画布发生变化；子候选已保留，请从提案列表重新核对。");
      onCommitted(); resetMarkups();
      await onRevised(child.id);
    } catch (cause) { if (active.current) setError(String(cause)); } finally { onBusy(false); }
  }}>
    <p>基于正在核对的候选继续修改，保留此前修改；预览后再采用。</p>
    <p className="text-muted-foreground">未提交输入保留在当前画布页内，切换对象后可返回继续；刷新或关闭画布页后清除。</p>
    {children}
    {(candidate.review_markups ?? []).some(markup => markup.status === "open") && <fieldset disabled={busy}><legend>本次修改回应的意见（不勾选则不关联）</legend>{candidate.review_markups!.filter(markup => markup.status === "open").map(markup => <label key={markup.id} className="block"><input type="checkbox" checked={markupIds.includes(markup.id)} onChange={event => setMarkupIds(current => event.target.checked ? [...current, markup.id] : current.filter(id => id !== markup.id))} />{markup.note}</label>)}</fieldset>}
    <Button type="submit" size="sm" disabled={busy || !valid || !candidate.document_sha256}>{submitLabel}</Button>
    <Button type="button" size="sm" variant="outline" className="ml-2" disabled={busy} onClick={() => { onCommitted(); resetMarkups(); setError(""); }}>还原本表单输入</Button>
    {error && <p role="alert" className="text-destructive">{error}</p>}
  </form></details>;
}
