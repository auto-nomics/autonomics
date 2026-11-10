import { useEffect, useMemo, useState } from "react";
import type { MarkupSummary } from "./canvas-markup-render";
import { CanvasPreview } from "./canvas-preview";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
type Proposal = { id: string; summary: string; base_revision: number; status: string; document: ExcalidrawInitialDataState; review_markups?: MarkupSummary[] };
export function CanvasProposals({ projectId, canvasId, isDirty, onAccepted, request }: { projectId: string; canvasId: string; isDirty: () => boolean; onAccepted: () => void; request?: { id: string } }) {
  const [items, setItems] = useState<Proposal[]>([]);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Proposal | null>(null);
  const [original, setOriginal] = useState<ExcalidrawInitialDataState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [resolveMarkups, setResolveMarkups] = useState(false);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposals`;
  useEffect(() => {
    if (!request) return;
    let cancelled = false;
    setOpen(true); setBusy(true); setError(""); setSelected(null); setOriginal(null); setResolveMarkups(false);
    void (async () => {
      try {
        const items = (await gatewayRequest("GET", path) as Proposal[]).filter(item => item.status === "pending");
        if (cancelled) return; setItems(items);
        const item = items.find(item => item.id === request.id);
        if (!item) throw new Error("提案已不存在或已处理，请刷新列表。");
        setSelected(item);
        const revision = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/revisions/${item.base_revision}`) as { document: ExcalidrawInitialDataState; review_markups?: MarkupSummary[] };
        if (!cancelled) setOriginal(revision.document);
      } catch (cause) { if (!cancelled) { setSelected(null); setError(String(cause)); } }
      finally { if (!cancelled) setBusy(false); }
    })();
    return () => { cancelled = true; };
  }, [request, path, projectId, canvasId]);
  const summary = useMemo(() => {
    if (!original || !selected) return "";
    const before = new Map((original.elements ?? []).filter(element => !element.isDeleted).map(element => [element.id, element]));
    const after = new Map((selected.document.elements ?? []).filter(element => !element.isDeleted).map(element => [element.id, element]));
    const added = [...after.keys()].filter(id => !before.has(id)).length;
    const removed = [...before.keys()].filter(id => !after.has(id)).length;
    const changed = [...after].filter(([id, element]) => before.has(id) && JSON.stringify(element) !== JSON.stringify(before.get(id))).length;
    return `新增 ${added} · 修改 ${changed} · 删除 ${removed}`;
  }, [original, selected]);
  async function select(item: Proposal) {
    setSelected(item); setResolveMarkups(false); setOriginal(null); setBusy(true); setError("");
    try {
      const revision = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/revisions/${item.base_revision}`) as { document: ExcalidrawInitialDataState; review_markups?: MarkupSummary[] };
      setOriginal(revision.document);
    } catch (cause) { setSelected(null); setError(String(cause)); } finally { setBusy(false); }
  }
  async function decide(action: "accept" | "reject") {
    if (!selected || busy) return;
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
      try { setItems((await gatewayRequest("GET", path) as Proposal[]).filter(item => item.status === "pending")); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>查看 Agent 提案</Button>
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[95vw] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between border-b p-3"><DialogTitle>Agent 修改预览</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => { setOpen(false); setSelected(null); }}>关闭预览</Button></div>
      {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
      <div className="flex min-h-0 flex-1"><aside className="w-64 shrink-0 overflow-auto border-r p-2">
        {!items.length && <p className="p-2 text-sm text-muted-foreground">{busy ? "加载中…" : "没有待审阅提案"}</p>}
        {items.map(item => <Button key={item.id} variant={selected?.id === item.id ? "secondary" : "ghost"} className="h-auto w-full justify-start whitespace-normal py-2 text-left" disabled={busy} onClick={() => void select(item)}>{item.summary} · v{item.base_revision}</Button>)}
        {Boolean(selected?.review_markups?.length) && <section aria-label="提案关联标记" className="mt-3 border-t pt-3">
          <p className="mb-2 text-sm font-medium">回应的审阅意见</p>
          {selected!.review_markups!.map(markup => <article key={markup.id} className="mb-2 rounded border p-2 text-xs"><p className="whitespace-pre-wrap">{markup.note}</p><p className="mt-1 text-muted-foreground">v{markup.at_revision} · {markup.status === "resolved" ? "已处理" : "待处理"}{markup.at_revision !== selected!.base_revision ? " · 历史意见，请核对当前图稿" : ""}</p></article>)}
        </section>}
      </aside><div className="flex min-h-0 min-w-0 flex-1 flex-col">{selected ? <>
        <p className="border-b p-2 text-xs" role="status">{summary || "加载原始版本…"}</p>
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-px bg-border md:grid-cols-2">
          <section aria-label="修改前画布" className="flex min-h-0 min-w-0 flex-col bg-background"><p className="border-b p-2 text-xs">修改前 · v{selected.base_revision}</p><div className="min-h-0 flex-1">{original && <CanvasPreview key={`${selected.id}:before`} document={original} />}</div></section>
          <section aria-label="候选画布" className="flex min-h-0 min-w-0 flex-col bg-background"><p className="border-b p-2 text-xs">候选修改</p><div className="min-h-0 flex-1"><CanvasPreview key={`${selected.id}:after`} document={selected.document} /></div></section>
        </div>
      </> : <p className="p-4 text-sm text-muted-foreground">选择一个提案对照修改前后的画布。</p>}</div></div>
      {selected && <div className="flex items-center justify-end gap-2 border-t p-3"><div className="mr-auto flex flex-col gap-2 text-xs"><span className="text-muted-foreground">采用时会检查服务端版本；请核对科学内容。</span>{Boolean(selected.review_markups?.length) && <label className="flex items-center gap-2"><input type="checkbox" checked={resolveMarkups} disabled={busy} onChange={event => setResolveMarkups(event.target.checked)} />采用时同时完成关联标记（{selected.review_markups!.length} 组）</label>}</div><Button variant="outline" disabled={busy} onClick={() => void decide("reject")}>拒绝提案</Button><Button disabled={busy} onClick={() => void decide("accept")}>采用提案</Button></div>}
    </DialogContent>
  </Dialog>;
}
