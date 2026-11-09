import { useState } from "react";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { CanvasPreview } from "./canvas-preview";
import { saveCanvas } from "./canvas-store";
type Summary = { revision: number; title: string; created_at: string };
type Revision = Summary & { document: ExcalidrawInitialDataState };
export function CanvasHistory({ projectId, canvasId, getRevision, isDirty, onRestored }: { projectId: string; canvasId: string; getRevision: () => number | undefined; isDirty: () => boolean; onRestored: () => void }) {
  const [open, setOpen] = useState(false);
  const [head, setHead] = useState<number>();
  const [items, setItems] = useState<Summary[]>([]);
  const [selected, setSelected] = useState<Revision | null>(null);
  const [more, setMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/revisions`;
  async function load(before?: number) {
    setBusy(true); setError("");
    try { const page = await gatewayRequest("GET", `${path}?limit=20${before ? `&before=${before}` : ""}`) as Summary[]; setItems(items => before ? [...items, ...page] : page); setMore(page.length === 20); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) { setOpen(value); if (!value) setSelected(null); } }}>
    <Button size="sm" variant="outline" disabled={busy || getRevision() === undefined} onClick={() => { setHead(getRevision()); setSelected(null); setOpen(true); void load(); }}>历史版本</Button>
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[95vw] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between border-b p-3"><DialogTitle>画布历史</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>关闭历史</Button></div>
      {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
      <div className="flex min-h-0 flex-1"><aside className="w-64 shrink-0 overflow-auto border-r p-2">
        {items.map(item => <Button key={item.revision} variant={selected?.revision === item.revision ? "secondary" : "ghost"} className="h-auto w-full justify-start whitespace-normal py-2 text-left" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); setSelected(null);
          try { setSelected(await gatewayRequest("GET", `${path}/${item.revision}`) as Revision); }
          catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>v{item.revision} · {item.title}</Button>)}
        {more && <Button variant="outline" size="sm" disabled={busy} onClick={() => void load(items.at(-1)?.revision)}>加载更早版本</Button>}
      </aside><section aria-label="历史画布" className="flex min-h-0 min-w-0 flex-1 flex-col">{selected ? <><p className="border-b p-2 text-xs">v{selected.revision} · {selected.created_at}</p><div className="min-h-0 flex-1"><CanvasPreview key={selected.revision} document={selected.document} /></div></> : <p role="status" className="p-4 text-sm text-muted-foreground">{busy ? "加载中…" : "选择一个版本查看完整画布。"}</p>}</section></div>
      <div className="flex items-center justify-between gap-2 border-t p-3"><p className="text-xs text-muted-foreground">恢复会创建新修订，原有历史仍保留。</p><Button disabled={busy || !selected || selected.revision === head} onClick={async () => {
        if (!selected || head === undefined || busy) return;
        if (isDirty()) { setError("当前画布有未保存修改，请先保存或重新加载。"); return; }
        setBusy(true); setError("");
        try { await saveCanvas({ id: canvasId, projectId, revision: head, title: selected.title, scene: JSON.stringify(selected.document), updatedAt: Date.now() }); onRestored(); }
        catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>恢复为新版本</Button></div>
    </DialogContent>
  </Dialog>;
}
