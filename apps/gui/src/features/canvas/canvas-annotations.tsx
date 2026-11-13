import { useEffect, useState } from "react";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
export type Annotation = { id: string; element_ids: string[]; note: string; status: string; at_revision: number };
export function CanvasAnnotations({ projectId, canvasId, getApi, getRevision, ensureSaved, preparing = false, canPrepare = false, onPrepare }: {
  projectId: string; canvasId: string; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>;
  preparing?: boolean; canPrepare?: boolean; onPrepare?: (annotation: Annotation) => Promise<boolean>;
}) {
  const [notes, setNotes] = useState<Annotation[]>([]);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/annotations`;
  useEffect(() => { let cancelled = false; void gatewayRequest("GET", path).then(items => { if (!cancelled) setNotes(items as Annotation[]); }).catch(cause => { if (!cancelled) setError(String(cause)); }); return () => { cancelled = true; }; }, [path]);
  const save = async (prepare: boolean) => {
      const api = getApi(); if (!api || busy || preparing) return;
      const targets = Object.keys(api.getAppState().selectedElementIds).filter(id => api.getAppState().selectedElementIds[id]);
      if (!targets.length) { setError("请先在画布上选择需要批注的对象。"); return; }
      setBusy(true); setError("");
      try {
        if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
        const added = await gatewayRequest("POST", path, { expected_revision: getRevision(), element_ids: targets, note: note.trim() }) as Annotation;
        setNotes(items => [...items, added]); setNote("");
        if (prepare && onPrepare && !await onPrepare(added)) setError("批注已保存，Agent 草稿未准备成功；请查看上方错误，修正后可从该批注重试。");
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }

  };
  return <section aria-label="对象批注" className="mt-4 flex flex-col gap-2 border-t pt-3">
    <p className="font-medium">对象批注</p>
    <p className="text-xs text-muted-foreground">选中画布对象后添加意见，批注不会进入图片导出。</p>
    <textarea aria-label="批注意见" disabled={busy || preparing} maxLength={4000} className="min-h-20 rounded border bg-background p-2" value={note} onChange={event => setNote(event.target.value)} />
    <Button size="sm" disabled={busy || preparing || !note.trim()} onClick={() => void save(false)}>批注选中对象</Button>
    {onPrepare && <><Button size="sm" disabled={busy || preparing || !canPrepare || !note.trim()} onClick={() => void save(true)}>批注并准备 Agent 修改</Button><p className="text-xs text-muted-foreground">保存批注并将此处修改要求放入上方所选 Agent 的草稿，供你检查并发送。</p></>}
    {error && <p role="alert" className="text-destructive">{error}</p>}
    {notes.map(item => <article key={item.id} className="rounded border p-2"><p className="whitespace-pre-wrap">{item.note}</p><p className="mt-1 text-xs text-muted-foreground">v{item.at_revision} · {item.status === "resolved" ? "已处理" : "待处理"}</p><div className="mt-2 flex flex-wrap gap-1"><Button size="sm" variant="outline" disabled={busy || preparing} onClick={() => {
      const api = getApi(); if (!api) return; const elements = api.getSceneElements().filter(element => item.element_ids.includes(element.id));
      if (!elements.length) { setError("批注关联的对象已不在当前画布中。"); return; }
      api.updateScene({ appState: { selectedElementIds: Object.fromEntries(elements.map(element => [element.id, true])) } }); api.scrollToContent(elements);
    }}>定位对象</Button>{item.status === "open" && onPrepare && <Button size="sm" variant="outline" disabled={busy || preparing || !canPrepare} onClick={async () => { setBusy(true); setError(""); try { if (!await onPrepare(item)) setError("Agent 草稿未准备成功，请查看上方错误并核对批注后重试。"); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>用此批注准备修改</Button>}{item.status !== "resolved" && <Button size="sm" variant="outline" disabled={busy || preparing} onClick={async () => {
      setBusy(true); setError(""); try { await gatewayRequest("POST", `${path}/${encodeURIComponent(item.id)}/resolve`); setNotes(items => items.map(current => current.id === item.id ? { ...current, status: "resolved" } : current)); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>标为已处理</Button>}</div></article>)}
  </section>;
}
