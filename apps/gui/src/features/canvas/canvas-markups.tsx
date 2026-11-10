import { useEffect, useRef, useState } from "react";
import { Excalidraw, sceneCoordsToViewportCoords, serializeAsJSON, viewportCoordsToSceneCoords } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { markupColor, renderMarkup, type MarkupPage, type MarkupPoint, type MarkupRecord, type MarkupStroke, type MarkupSummary } from "./canvas-markup-render";

type Draft = { key: string; revision: number; serialized: string; document: ExcalidrawInitialDataState };
export function CanvasMarkups({ projectId, canvasId, getApi, getRevision, ensureSaved }: {
  projectId: string; canvasId: string; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [items, setItems] = useState<MarkupSummary[]>([]), [selected, setSelected] = useState<MarkupRecord | null>(null);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [status, setStatus] = useState("all");
  const [draft, setDraft] = useState<Draft | null>(null), [note, setNote] = useState("");
  const [strokes, setStrokes] = useState<MarkupStroke[]>([]), [active, setActive] = useState<MarkupStroke | null>(null);
  const [navigating, setNavigating] = useState(false);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);
  const [mode, setMode] = useState<MarkupStroke["kind"]>("draw");
  const ink = useRef<MarkupStroke | null>(null), reviewApi = useRef<ExcalidrawImperativeAPI | null>(null), layer = useRef<SVGSVGElement>(null);
  const [, setViewport] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/markups`;
  const discard = () => !draft || !strokes.length && !ink.current && !note.trim() || window.confirm("放弃尚未保存的草图标记？");
  useEffect(() => {
    const resource = `canvas-markup:${projectId}:${canvasId}`;
    setDocumentDirty(resource, Boolean(draft && (strokes.length || active || note.trim())));
    return () => setDocumentDirty(resource, false);
  }, [projectId, canvasId, draft, strokes.length, active, note]);
  const load = async (before?: string, filter = status) => {
    const query = new URLSearchParams({ page: "true" });
    if (filter !== "all") query.set("status", filter);
    if (before) query.set("before", before);
    const page = await gatewayRequest("GET", `${path}?${query}`) as MarkupPage;
    setItems(previous => before ? [...previous, ...page.items.filter(item => !previous.some(existing => existing.id === item.id))] : page.items);
    setNextBefore(page.next_before);
  };
  async function start() {
    if (!discard()) return;
    setBusy(true); setError("");
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision(); if (!api || revision === undefined) throw new Error("画布尚未就绪");
      const serialized = serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
      reviewApi.current = null; fitted.current = false; setReady(false); ink.current = null; setActive(null); setNavigating(false); setStrokes([]); setNote(""); setSelected(null);
      setDraft({ key: crypto.randomUUID(), revision, serialized, document: { ...JSON.parse(serialized), scrollToContent: true } });
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  function scenePoint(event: React.PointerEvent<SVGSVGElement>): MarkupPoint | null {
    const api = reviewApi.current; if (!api) return null;
    const point = viewportCoordsToSceneCoords({ clientX: event.clientX, clientY: event.clientY }, api.getAppState());
    return [point.x, point.y];
  }
  function local(point: MarkupPoint) {
    const api = reviewApi.current, bounds = layer.current?.getBoundingClientRect();
    if (!api || !bounds) return point;
    const view = sceneCoordsToViewportCoords({ sceneX: point[0], sceneY: point[1] }, api.getAppState());
    return [view.x - bounds.left, view.y - bounds.top];
  }
  function renderStroke(stroke: MarkupStroke, index: number) {
    const points = stroke.points.map(local), a = points[0], b = points.at(-1)!;
    const style = { fill: "none", stroke: markupColor(stroke.kind), strokeWidth: 3 * (reviewApi.current?.getAppState().zoom.value ?? 1) };
    if (stroke.kind === "emphasis") return <ellipse key={index} {...style} cx={(a[0] + b[0]) / 2} cy={(a[1] + b[1]) / 2} rx={Math.abs(a[0] - b[0]) / 2} ry={Math.abs(a[1] - b[1]) / 2} />;
    if (stroke.kind === "cross") return <path key={index} {...style} d={`M${a.join(",")}L${b.join(",")}M${a[0]},${b[1]}L${b[0]},${a[1]}`} />;
    return <polyline key={index} {...style} strokeLinejoin="round" strokeLinecap="round" points={points.map(point => point.join(",")).join(" ")} />;
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy && (!value ? discard() : true)) { setOpen(value); if (!value) { setDraft(null); setSelected(null); ink.current = null; setActive(null); } } }}>
    <Button size="sm" variant="outline" disabled={busy || getRevision() === undefined} onClick={async () => { setOpen(true); try { await load(); } catch (cause) { setError(String(cause)); } }}>草图标记</Button>
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[95vw] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between border-b p-3"><DialogTitle>草图标记与审阅</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (discard()) { setOpen(false); setDraft(null); ink.current = null; } }}>关闭标记</Button></div>
      {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
      <div className="flex min-h-0 flex-1"><aside className="flex w-64 shrink-0 flex-col gap-2 overflow-auto border-r p-3">
        <Button size="sm" disabled={busy} onClick={() => void start()}>新建标记</Button>
        <p className="text-xs text-muted-foreground">笔迹与说明独立保存，不进入正式图稿或导出。可筛选待处理意见并加载更早记录。</p>
        <label className="text-sm">标记状态<select aria-label="标记状态" className="mt-1 w-full rounded border bg-background p-1" value={status} disabled={busy} onChange={async event => {
          const filter = event.target.value; setBusy(true); setError("");
          try { await load(undefined, filter); setStatus(filter); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}><option value="all">全部标记</option><option value="open">待处理</option><option value="resolved">已处理</option></select></label>
        {draft && <><p className="text-xs">正在审阅 v{draft.revision}</p><div className="flex flex-wrap gap-1">{([['draw', '补画'], ['cross', '划除'], ['emphasis', '强调']] as const).map(([kind, label]) => <Button key={kind} size="sm" variant={!navigating && mode === kind ? "default" : "outline"} disabled={busy} onClick={() => { setMode(kind); setNavigating(false); }}>{label}</Button>)}</div>
          <Button size="sm" variant={navigating ? "default" : "outline"} disabled={busy} onClick={() => setNavigating(value => !value)}>浏览与缩放</Button>
          <Button size="sm" variant="outline" disabled={busy || !strokes.length} onClick={() => setStrokes(items => items.slice(0, -1))}>撤销笔迹</Button>
          <label className="text-sm">修改说明<textarea aria-label="标记修改说明" maxLength={4000} value={note} onChange={event => setNote(event.target.value)} className="mt-1 min-h-24 w-full rounded border bg-background p-2" /></label>
          <Button size="sm" disabled={busy || !ready || !note.trim() || !strokes.length} onClick={async () => {
            const api = reviewApi.current, source = getApi(); if (!draft || !api || !source || busy) return;
            setBusy(true); setError("");
            try {
              const unchanged = () => getRevision() === draft.revision && serializeAsJSON(source.getSceneElements(), source.getAppState(), source.getFiles(), "local") === draft.serialized;
              if (!unchanged()) throw new Error("正式画布已发生变化，请重新建立标记。");
              const dataURL = await renderMarkup(api, strokes);
              if (!unchanged()) throw new Error("生成标记预览时画布发生变化，请重新建立标记。");
              const saved = await gatewayRequest("POST", path, { expected_revision: draft.revision, note: note.trim(), strokes, dataURL }) as MarkupSummary;
              setDraft(null); setStrokes([]); setActive(null); ink.current = null;
              setSelected({ ...saved, strokes, dataURL }); await load();
            } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
          }}>{busy ? "保存中…" : "保存草图标记"}</Button></>}
        {items.map(item => <article key={item.id} className="rounded border p-2"><p className="whitespace-pre-wrap text-sm">{item.note}</p><p className="my-1 text-xs text-muted-foreground">v{item.at_revision} · {item.stroke_count} 处标记 · {item.status === "resolved" ? "已处理" : "待处理"}</p><div className="flex gap-1"><Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          if (!discard()) return; setBusy(true); setError("");
          try { const record = await gatewayRequest("GET", `${path}/${encodeURIComponent(item.id)}`) as MarkupRecord; setDraft(null); setSelected(record); setStrokes([]); ink.current = null; setActive(null); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>查看标记</Button>{item.status !== "resolved" && <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); try { await gatewayRequest("POST", `${path}/${encodeURIComponent(item.id)}/resolve`); await load(); setSelected(current => current?.id === item.id ? { ...current, status: "resolved" } : current); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>完成标记</Button>}</div></article>)}
        {nextBefore && <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); try { await load(nextBefore); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>加载更早标记</Button>}
      </aside><section aria-label="草图标记预览" className="relative min-h-0 min-w-0 flex-1 bg-white">
        {draft ? <><Excalidraw key={draft.key} initialData={draft.document} excalidrawAPI={api => { reviewApi.current = api; }} viewModeEnabled zenModeEnabled handleKeyboardGlobally={false} langCode="zh-CN" onChange={(elements, state, files) => {
            setReady(Boolean(reviewApi.current) && elements.every(element => element.type !== "image" || Boolean(element.fileId && files[element.fileId])));
            if (elements.length && !fitted.current && reviewApi.current) { fitted.current = true; reviewApi.current.scrollToContent(elements, { fitToViewport: true, maxZoom: 1 }); }
            setViewport([state.zoom.value, state.scrollX, state.scrollY, state.offsetLeft, state.offsetTop, state.width, state.height].join(","));
          }} />
          <svg ref={layer} role="application" aria-label="草图标记区域" style={{ pointerEvents: navigating ? "none" : undefined }} className="absolute inset-0 z-20 h-full w-full touch-none cursor-crosshair" onPointerDown={event => {
            if (busy || !ready || event.button !== 0) return; event.preventDefault(); const point = scenePoint(event); if (!point) return;
            if (strokes.length >= 100) { setError("每组最多 100 处标记，请先保存。"); return; }
            event.currentTarget.setPointerCapture(event.pointerId); ink.current = { kind: mode, points: [point, point] }; setActive(ink.current);
          }} onPointerMove={event => {
            if (!ink.current) return; const point = scenePoint(event); if (!point) return;
            if (ink.current.kind === "draw") { if (ink.current.points.length >= 2000) return; ink.current = { ...ink.current, points: [...ink.current.points, point] }; }
            else ink.current = { ...ink.current, points: [ink.current.points[0], point] };
            setActive(ink.current);
          }} onPointerCancel={() => { ink.current = null; setActive(null); }} onPointerUp={event => {
            event.preventDefault(); const stroke = ink.current; ink.current = null; setActive(null); if (!stroke) return;
            const point = scenePoint(event); if (point) stroke.points[stroke.points.length - 1] = point;
            const first = stroke.points[0], last = stroke.points.at(-1)!;
            if (!stroke.points.some(point => Math.hypot(point[0] - first[0], point[1] - first[1]) > 2) || stroke.kind !== "draw" && (Math.abs(first[0] - last[0]) < 2 || Math.abs(first[1] - last[1]) < 2)) return;
            if (strokes.reduce((sum, item) => sum + item.points.length, 0) + stroke.points.length > 10000) { setError("笔迹过多，请先保存现有标记。"); return; }
            setStrokes(items => [...items, stroke]);
          }}><rect width="100%" height="100%" fill="transparent" />{[...strokes, ...(active ? [active] : [])].map(renderStroke)}</svg></> : selected ? <div className="flex h-full flex-col"><p className="border-b bg-background p-2 text-sm">v{selected.at_revision} 的标记预览{selected.at_revision !== getRevision() ? " · 历史版本，请与当前画布核对" : ""}</p><img alt="已保存的草图标记" src={selected.dataURL} className="min-h-0 w-full flex-1 object-contain" /></div> : <p className="p-4 text-sm text-slate-600">新建一组标记，或查看已保存的审阅记录。</p>}
      </section></div>
    </DialogContent>
  </Dialog>;
}
