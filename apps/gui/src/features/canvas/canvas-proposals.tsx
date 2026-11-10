import { CanvasAssetComparison } from "./canvas-asset-comparison";
import { CanvasCandidateObjects } from "./canvas-candidate-objects";
import { useEffect, useMemo, useState } from "react";
import type { MarkupSummary } from "./canvas-markup-render";
import { useProposalNotice, type ProposalNotice } from "./canvas-proposal-notice";
import { CanvasProposalFeedback } from "./canvas-proposal-feedback";
import { textOverlapChanges, textOverlaps } from "./canvas-text-overlaps";
import { proposalDiff } from "./canvas-proposal-diff";
import { CanvasPreview, type PreviewFocus } from "./canvas-preview";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
type Proposal = { id: string; summary: string; base_revision: number; status: string; document: ExcalidrawInitialDataState; review_markups?: MarkupSummary[]; document_sha256?: string; parent_proposal_id?: string; parent_document_sha256?: string };
type ProposalBrief = Pick<Proposal, "id" | "summary" | "base_revision" | "parent_proposal_id"> & { summary_truncated?: boolean };
type ProposalPage = { items: ProposalBrief[]; next_before: string | null };
export function CanvasProposals({ projectId, canvasId, isDirty, onAccepted, request }: { projectId: string; canvasId: string; isDirty: () => boolean; onAccepted: () => void; request?: { id: string } }) {
  const [items, setItems] = useState<ProposalBrief[]>([]);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Proposal | null>(null);
  const [observedHead, setObservedHead] = useState<number | null>(null);
  const [original, setOriginal] = useState<ExcalidrawInitialDataState | null>(null);
  const [parent, setParent] = useState<Proposal | null>(null);
  const [focus, setFocus] = useState<PreviewFocus>(null);
  const [compareParent, setCompareParent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [resolveMarkups, setResolveMarkups] = useState(false);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposals`;
  const notice = useProposalNotice(`/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposal-summary`);
  async function loadPage(before?: string) {
    const page = await gatewayRequest("GET", `${path.replace(/proposals$/, "proposal-index")}?limit=20${before ? `&before=${encodeURIComponent(before)}` : ""}`) as ProposalPage;
    setItems(current => before ? [...current, ...page.items.filter(item => !current.some(previous => previous.id === item.id))] : page.items);
    setNextBefore(page.next_before);
    return page;
  }
  async function readCandidate(id: string) {
    const item = await gatewayRequest("GET", `${path}/${encodeURIComponent(id)}`) as Proposal;
    if (item.status !== "pending") throw new Error("提案已处理，请刷新列表。");
    return item;
  }
  async function openLatest() {
    if (!notice?.latest || busy) return;
    setOpen(true); setBusy(true); setError(""); setSelected(null); setOriginal(null); setParent(null); setFocus(null); setCompareParent(false);
    try {
      await loadPage();
      await select({id:notice.latest.id,summary:notice.latest.summary,base_revision:notice.latest.base_revision});
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  async function loadComparison(item: Proposal) {
    const [revision, parent, head] = await Promise.all([
      gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/revisions/${item.base_revision}`).then(value => value as { document: ExcalidrawInitialDataState }),
      item.parent_proposal_id ? gatewayRequest("GET", `${path}/${encodeURIComponent(item.parent_proposal_id)}`).then(value => value as Proposal) : Promise.resolve(null),
      gatewayRequest("GET", path.replace(/proposals$/, "proposal-summary")).then(value => value as ProposalNotice),
    ]);
    return { revision, parent, head };
  }
  useEffect(() => {
    if (!request) return;
    let cancelled = false;
    setOpen(true); setBusy(true); setError(""); setSelected(null); setOriginal(null); setParent(null); setCompareParent(false); setFocus(null); setResolveMarkups(false);
    void (async () => {
      try {
        const page = await gatewayRequest("GET", `${path.replace(/proposals$/, "proposal-index")}?limit=20`) as ProposalPage;
        const item = await readCandidate(request.id);
        if (cancelled) return;
        setItems(page.items.some(entry => entry.id === item.id) ? page.items : [item,...page.items]);
        setNextBefore(page.next_before); setSelected(item);
        const loaded = await loadComparison(item);
        if (!cancelled) { setOriginal(loaded.revision.document); setParent(loaded.parent); setObservedHead(loaded.head.current_revision); }

      } catch (cause) { if (!cancelled) { setSelected(null); setError(String(cause)); } }
      finally { if (!cancelled) setBusy(false); }
    })();
    return () => { cancelled = true; };
  }, [request, path, projectId, canvasId]);
  const currentRevision = observedHead === null && !notice ? null : Math.max(observedHead ?? 0, notice?.current_revision ?? 0);
  const outdated = Boolean(selected && currentRevision !== null && selected.base_revision !== currentRevision);
  const comparison = compareParent && parent ? parent.document : original;
  const diff = useMemo(() => comparison && selected ? proposalDiff(comparison, selected.document) : null, [comparison, selected]);
  const overlaps = useMemo(() => selected ? textOverlaps(selected.document) : null, [selected]);
  const overlapChanges = useMemo(() => comparison && selected ? textOverlapChanges(comparison,selected.document) : null, [comparison,selected]);
  const summary = diff ? `新增 ${diff.added} · 修改 ${diff.changed} · 删除 ${diff.removed} · 素材文件 ${diff.fileCount}` : "";
  async function select(brief: ProposalBrief) {
    setSelected(null); setObservedHead(null); setResolveMarkups(false); setOriginal(null); setParent(null); setCompareParent(false); setFocus(null); setBusy(true); setError("");
    try {
      const item = await readCandidate(brief.id);
      setSelected(item);
      setItems(current => current.some(entry => entry.id === item.id) ? current : [item,...current]);
      const loaded = await loadComparison(item);
      setOriginal(loaded.revision.document); setParent(loaded.parent); setObservedHead(loaded.head.current_revision);
    } catch (cause) { setSelected(null); setError(String(cause)); } finally { setBusy(false); }
  }
  async function decide(action: "accept" | "reject") {
    if (!selected || busy) return;
    if (action === "accept" && outdated) { setError("候选基础版本已过期，请核对当前正式图稿后重新生成提案。"); return; }
    if (action === "accept" && isDirty()) { setError("当前画布有未保存修改。请先保存或重新加载，再检查提案版本。"); return; }
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", `${path}/${encodeURIComponent(selected.id)}/${action}${action === "accept" && resolveMarkups ? "?resolve_markups=true" : ""}`);
      if (action === "accept") onAccepted();
      else { setItems(current => current.filter(item => item.id !== selected.id)); setSelected(null); }
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) { setOpen(value); if (!value) setSelected(null); } }}>
    <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
      setOpen(true); setBusy(true); setError("");
      try { await loadPage(); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>查看 Agent 提案</Button>
    {notice && <span aria-label="待审阅提案数量" className="text-xs text-muted-foreground">待审阅 {notice.total}{notice.stale > 0 ? `（旧基础版本 ${notice.stale}）` : ""}</span>}
    {notice?.latest && <Button size="sm" variant="outline" disabled={busy} className="max-w-64 truncate" title={notice.latest.summary} onClick={() => void openLatest()}>{notice.current > 0 ? "审阅最新候选" : "审阅历史候选"}</Button>}
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[95vw] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between border-b p-3"><DialogTitle>Agent 修改预览</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => { setOpen(false); setSelected(null); }}>关闭预览</Button></div>
      {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
      <div className="flex min-h-0 flex-1"><aside className="w-64 shrink-0 overflow-auto border-r p-2">
        {!items.length && <p className="p-2 text-sm text-muted-foreground">{busy ? "加载中…" : "没有待审阅提案"}</p>}
        {items.map(item => <Button key={item.id} variant={selected?.id === item.id ? "secondary" : "ghost"} className="h-auto w-full justify-start whitespace-normal py-2 text-left" disabled={busy} onClick={() => void select(item)}>{item.summary}{item.summary_truncated ? "…" : ""} · v{item.base_revision}{item.parent_proposal_id ? " · 修订" : ""}</Button>)}
        {nextBefore && <Button size="sm" variant="outline" disabled={busy} onClick={async () => { setBusy(true); setError(""); try { await loadPage(nextBefore); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>加载更早提案</Button>}
        {selected && <CanvasCandidateObjects key={selected.id} document={selected.document} focusedId={focus?.id} onFocus={id => setFocus(current => id ? { id, request: (current?.request ?? 0) + 1 } : null)} />}
        {Boolean(selected?.review_markups?.length) && <section aria-label="提案关联标记" className="mt-3 border-t pt-3">
          <p className="mb-2 text-sm font-medium">回应的审阅意见</p>
          {selected!.review_markups!.map(markup => <article key={markup.id} className="mb-2 rounded border p-2 text-xs"><p className="whitespace-pre-wrap">{markup.note}</p><p className="mt-1 text-muted-foreground">v{markup.at_revision} · {markup.status === "resolved" ? "已处理" : "待处理"}{markup.at_revision !== selected!.base_revision ? " · 历史意见，请核对当前图稿" : ""}</p></article>)}
        </section>}
      </aside><div className="flex min-h-0 min-w-0 flex-1 flex-col">{selected ? <>
        <p aria-label="候选版本" className="shrink-0 border-b p-2 text-xs" role="status">候选基于 v{selected.base_revision} · 当前正式 {currentRevision === null ? "核对中…" : `v${currentRevision}`}{outdated ? " · 基础版本已过期；可检查和拒绝，继续修改须先核对当前正式图稿。" : ""}</p>
        {parent && <section aria-label="提案修订来源" className="flex flex-wrap items-center gap-2 border-b p-2 text-xs"><span>基于「{parent.summary}」修订 · 来源{parent.status === "pending" ? "待审阅" : parent.status === "accepted" ? "已采用" : "已拒绝"}；本提案独立审阅</span><Button size="sm" variant={!compareParent ? "secondary" : "outline"} disabled={busy} onClick={() => setCompareParent(false)}>对照正式版本</Button><Button size="sm" variant={compareParent ? "secondary" : "outline"} disabled={busy} onClick={() => setCompareParent(true)}>对照上一版候选</Button></section>}
        <p className="border-b p-2 text-xs" role="status">{summary || "加载原始版本…"}</p>
        {overlaps && (overlaps.count > 0 || overlaps.skipped > 0 || (overlapChanges?.counts.resolved ?? 0) > 0) && <details className="shrink-0 border-b text-xs" aria-label="候选文字重叠检查"><summary className="cursor-pointer p-2">文字边界重叠 {overlaps.count} 对 · 已检查 {overlaps.checked}/{overlaps.total}</summary><div className="max-h-40 overflow-auto space-y-2 p-2">{overlapChanges && <p aria-label="文字重叠变化">相对当前对照：新增 {overlapChanges.counts.introduced} · 仍有 {overlapChanges.counts.persisting} · 消除 {overlapChanges.counts.resolved} · 未确定 {overlapChanges.counts.unknown}。对照未检查 {overlapChanges.before.skipped} 段。</p>}<p>仅检查未旋转文字的矩形边界；空白区域也可能重叠。请结合预览判断，提示不阻止采用。</p>{overlaps.pairs.map((pair,index) => <p key={index} className="break-all">{pair.first} ↔ {pair.second} <Button size="sm" variant="outline" aria-label={`定位重叠文字 ${pair.first}`} onClick={() => setFocus(current => ({id:pair.first,request:(current?.request ?? 0)+1}))}>定位第一段</Button> <Button size="sm" variant="outline" aria-label={`定位重叠文字 ${pair.second}`} onClick={() => setFocus(current => ({id:pair.second,request:(current?.request ?? 0)+1}))}>定位第二段</Button></p>)}{overlaps.skipped > 0 && <p>有 {overlaps.skipped} 段文字因旋转、无效边界、透明或数量上限未检查。</p>}{overlaps.truncated && <p>仅显示前 50 对。</p>}</div></details>}
        {diff && <details className="shrink-0 border-b text-xs" aria-label="逐对象修改明细"><summary className="cursor-pointer p-2">逐对象修改明细</summary><div className="max-h-56 overflow-auto p-2"><Button size="sm" variant="outline" className="mb-2" onClick={() => setFocus(null)}>查看整图</Button>
          {diff.objects.map(object => <article key={object.id} className="mb-2 rounded border p-2"><p className="mb-1 break-all font-medium">{object.kind === "added" ? "新增" : object.kind === "removed" ? "删除" : "修改"} · {object.type} · {object.id} <Button size="sm" variant="outline" onClick={() => setFocus(current => ({ id: object.id, request: (current?.request ?? 0) + 1 }))} aria-label={`定位对象 ${object.id}`}>定位对象</Button></p><table className="w-full table-fixed"><thead><tr><th className="w-1/4 text-left">字段</th><th className="text-left">修改前</th><th className="text-left">候选值</th></tr></thead><tbody>{object.fields.map(field => <tr key={field.field}><td className="break-all align-top">{field.field}</td><td className="whitespace-pre-wrap break-all align-top">{field.before}</td><td className="whitespace-pre-wrap break-all align-top">{field.after}</td></tr>)}</tbody></table>{object.fieldsTruncated && <p>字段较多，明细已截断，请结合源文件核对。</p>}</article>)}
          {diff.files.map(file => <p key={file.id} className="break-all">素材文件 · {file.id} · {file.kind === "added" ? "新增" : file.kind === "removed" ? "删除" : "内容或属性已修改"}</p>)}
          {diff.orderChanged && <p>对象前后层级顺序已修改。</p>}{diff.appStateChanged && <p>画布设置已修改。</p>}
          {(diff.truncated || diff.filesTruncated) && <p>修改较多，明细已截断，请结合源文件核对。</p>}
          {!diff.objects.length && !diff.fileCount && !diff.orderChanged && !diff.appStateChanged && <p>没有可见内容或素材文件修改。</p>}
        </div></details>}
        {focus && comparison && <CanvasAssetComparison key={selected.id} before={comparison} after={selected.document} focusedId={focus.id} comparisonLabel={compareParent && parent ? "上一版候选" : `正式版本 v${selected.base_revision}`} />}
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-px bg-border md:grid-cols-2">
          <section aria-label="修改前画布" className="flex min-h-0 min-w-0 flex-col bg-background"><p className="border-b p-2 text-xs">{compareParent && parent ? `上一版候选 · ${parent.summary}` : `修改前 · v${selected.base_revision}`}</p><div className="min-h-0 flex-1">{comparison && <CanvasPreview key={`${selected.id}:before:${compareParent}`} document={comparison} focus={focus} />}</div></section>
          <section aria-label="候选画布" className="flex min-h-0 min-w-0 flex-col bg-background"><p className="border-b p-2 text-xs">候选修改</p><div className="min-h-0 flex-1"><CanvasPreview key={`${selected.id}:after`} document={selected.document} focus={focus} /></div></section>
        </div>
      </> : <p className="p-4 text-sm text-muted-foreground">选择一个提案对照修改前后的画布。</p>}</div></div>
      {selected && !outdated && <CanvasProposalFeedback key={selected.id} projectId={projectId} canvasId={canvasId} candidate={selected} focusedId={focus?.id} changes={diff?.objects ?? []} comparison={compareParent && parent ? { kind: "parent_candidate", proposal_id: parent.id, document_sha256: parent.document_sha256 } : { kind: "formal_revision", revision: selected.base_revision }} isDirty={isDirty} onPrepared={() => { setOpen(false); setSelected(null); }} />}
      {selected && <div className="flex items-center justify-end gap-2 border-t p-3"><div className="mr-auto flex flex-col gap-2 text-xs"><span className="text-muted-foreground">采用时会检查服务端版本；请核对科学内容。</span>{Boolean(selected.review_markups?.length) && <label className="flex items-center gap-2"><input type="checkbox" checked={resolveMarkups} disabled={busy} onChange={event => setResolveMarkups(event.target.checked)} />采用时同时完成关联标记（{selected.review_markups!.length} 组）</label>}</div><Button variant="outline" disabled={busy} onClick={() => void decide("reject")}>拒绝提案</Button><Button disabled={busy || outdated || currentRevision === null} onClick={() => void decide("accept")}>采用提案</Button></div>}
    </DialogContent>
  </Dialog>;
}
