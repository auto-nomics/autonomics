import { CanvasPreview } from "./canvas-preview";
import { useCanvasAgentHandoff, type ReviewedMarkupScene, type CanvasAgentHandoffProps } from "./use-canvas-agent-handoff";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Excalidraw, sceneCoordsToViewportCoords, serializeAsJSON, viewportCoordsToSceneCoords } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { markupColor, renderMarkup, type MarkupPage, type MarkupPoint, type MarkupRecord, type MarkupStroke, type MarkupSummary } from "./canvas-markup-render";

type Draft = { key: string; revision: number; serialized: string; document: ExcalidrawInitialDataState };
export function CanvasMarkups(props: CanvasAgentHandoffProps & { agentPath?: string }) {
  const { t } = useTranslation("canvas");
  const { projectId, canvasId, getApi, getRevision, ensureSaved } = props;
  const target = props.agentPath ?? "";
  const handoff = useCanvasAgentHandoff({ ...props, target });
  const [open, setOpen] = useState(false), [working, setBusy] = useState(false), [error, setError] = useState("");
  const busy = working || handoff.busy;
  const [items, setItems] = useState<MarkupSummary[]>([]), [selected, setSelected] = useState<MarkupRecord | null>(null);
  const [currentReview, setCurrentReview] = useState<(ReviewedMarkupScene & { markupId: string; key: string; document: ExcalidrawInitialDataState }) | null>(null);
  const [showCurrent, setShowCurrent] = useState(false), [historyConfirmed, setHistoryConfirmed] = useState(false);
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
  const discard = () => !draft || !strokes.length && !ink.current && !note.trim() || window.confirm(t("markups.discardConfirm"));
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
    setBusy(true); setError(""); handoff.clearError();
    try {
      if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
      const api = getApi(), revision = getRevision(); if (!api || revision === undefined) throw new Error(t("markups.notReady"));
      const serialized = serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
      setCurrentReview(null); setHistoryConfirmed(false); setShowCurrent(false);
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
  async function save(prepareAfter: boolean) {
            const api = reviewApi.current, source = getApi(); if (!draft || !api || !source || busy) return;
            setBusy(true); setError(""); handoff.clearError();
            try {
              const unchanged = () => getRevision() === draft.revision && serializeAsJSON(source.getSceneElements(), source.getAppState(), source.getFiles(), "local") === draft.serialized;
              if (!unchanged()) throw new Error(t("markups.canvasChangedRebuild"));
              const dataURL = await renderMarkup(api, strokes);
              if (!unchanged()) throw new Error(t("markups.previewChanged"));
              const saved = await gatewayRequest("POST", path, { expected_revision: draft.revision, note: note.trim(), strokes, dataURL }) as MarkupSummary;
              setDraft(null); setStrokes([]); setActive(null); ink.current = null;
              setSelected({ ...saved, strokes, dataURL }); await load();
              if (prepareAfter) {
                if (await handoff.prepare(saved.note, undefined, saved)) setOpen(false);
                else setError(t("markups.savedDraftFailed"));
              }
            } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  async function prepareSaved(item: MarkupSummary) {
    if (!discard()) return;
    setBusy(true); setError(""); handoff.clearError();
    try {
      if (await handoff.prepare(item.note, undefined, item, historyConfirmed && currentReview?.markupId === item.id ? currentReview : undefined)) { setDraft(null); setStrokes([]); setActive(null); ink.current = null; setOpen(false); }
    } finally { setBusy(false); }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy && (!value ? discard() : true)) { setOpen(value); if (!value) { setDraft(null); setSelected(null); ink.current = null; setActive(null); } } }}>
    <Button size="sm" variant="outline" disabled={busy || getRevision() === undefined} onClick={async () => { setOpen(true); try { await load(); } catch (cause) { setError(String(cause)); } }}>{t("markups.button")}</Button>
    <DialogContent showCloseButton={false} className="flex h-[90vh] w-[95vw] max-w-none flex-col gap-0 overflow-hidden border bg-background p-0 sm:max-w-none">
      <div className="flex items-center justify-between border-b p-3"><DialogTitle>{t("markups.title")}</DialogTitle><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (discard()) { setOpen(false); setDraft(null); ink.current = null; } }}>{t("markups.close")}</Button></div>
      {(error || handoff.error) && <p role="alert" className="p-2 text-sm text-destructive">{[error, handoff.error].filter(Boolean).join("；")}</p>}
      <div className="flex min-h-0 flex-1"><aside className="flex w-64 shrink-0 flex-col gap-2 overflow-auto border-r p-3">
        <Button size="sm" disabled={busy} onClick={() => void start()}>{t("markups.newButton")}</Button>
        <p className="text-xs text-muted-foreground">{t("markups.hint1")}</p>
        <p className="text-xs text-muted-foreground">{t("markups.hint2")}</p>
        <label className="text-sm">{t("markups.statusLabel")}<select aria-label={t("markups.statusLabel")} className="mt-1 w-full rounded border bg-background p-1" value={status} disabled={busy} onChange={async event => {
          const filter = event.target.value; setBusy(true); setError(""); handoff.clearError();
          try { await load(undefined, filter); setStatus(filter); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}><option value="all">{t("markups.filterAll")}</option><option value="open">{t("markups.filterOpen")}</option><option value="resolved">{t("markups.filterResolved")}</option></select></label>
        {draft && <><p className="text-xs">{t("markups.reviewing", { revision: draft.revision })}</p><div className="flex flex-wrap gap-1">{([['draw', t("markups.modeDraw")], ['cross', t("markups.modeCross")], ['emphasis', t("markups.modeEmphasis")]] as const).map(([kind, label]) => <Button key={kind} size="sm" variant={!navigating && mode === kind ? "default" : "outline"} disabled={busy} onClick={() => { setMode(kind); setNavigating(false); }}>{label}</Button>)}</div>
          <Button size="sm" variant={navigating ? "default" : "outline"} disabled={busy} onClick={() => setNavigating(value => !value)}>{t("markups.browse")}</Button>
          <Button size="sm" variant="outline" disabled={busy || !strokes.length} onClick={() => setStrokes(items => items.slice(0, -1))}>{t("markups.undo")}</Button>
          <label className="text-sm">{t("markups.noteLabel")}<textarea aria-label={t("markups.noteFieldLabel")} disabled={busy} maxLength={4000} value={note} onChange={event => setNote(event.target.value)} className="mt-1 min-h-24 w-full rounded border bg-background p-2" /></label>
          <Button size="sm" disabled={busy || !ready || !note.trim() || !strokes.length} onClick={() => void save(false)}>{busy ? t("markups.saving") : t("markups.save")}</Button>
          <Button size="sm" disabled={busy || !handoff.targetAvailable || !ready || !note.trim() || !strokes.length} onClick={() => void save(true)}>{t("markups.saveAndPrepare")}</Button></>}
        {items.map(item => <article key={item.id} className="rounded border p-2"><p className="whitespace-pre-wrap text-sm">{item.note}</p><p className="my-1 text-xs text-muted-foreground">{t("markups.itemMeta", { revision: item.at_revision, count: item.stroke_count, status: item.status === "resolved" ? t("markups.statusResolved") : t("markups.statusOpen") })}</p><div className="flex flex-wrap gap-1"><Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          if (!discard()) return; setBusy(true); setError(""); handoff.clearError();
          try { const record = await gatewayRequest("GET", `${path}/${encodeURIComponent(item.id)}`) as MarkupRecord; setDraft(null); setSelected(record); setCurrentReview(null); setHistoryConfirmed(false); setShowCurrent(false); setStrokes([]); ink.current = null; setActive(null); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>{t("markups.view")}</Button>{item.status === "open" && <Button size="sm" variant="outline" disabled={busy || !handoff.targetAvailable || item.at_revision !== getRevision() && !(selected?.id === item.id && historyConfirmed && currentReview?.markupId === item.id && currentReview.revision === getRevision())} onClick={() => void prepareSaved(item)}>{t("markups.prepare")}</Button>}{item.status !== "resolved" && <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); handoff.clearError(); try { await gatewayRequest("POST", `${path}/${encodeURIComponent(item.id)}/resolve`); await load(); setSelected(current => current?.id === item.id ? { ...current, status: "resolved" } : current); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>{t("markups.resolve")}</Button>}</div></article>)}
        {nextBefore && <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); handoff.clearError(); try { await load(nextBefore); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>{t("markups.loadEarlier")}</Button>}
      </aside><section aria-label={t("markups.previewLabel")} className="relative min-h-0 min-w-0 flex-1 bg-white">
        {draft ? <><Excalidraw key={draft.key} initialData={draft.document} excalidrawAPI={api => { reviewApi.current = api; }} viewModeEnabled zenModeEnabled handleKeyboardGlobally={false} langCode="zh-CN" onChange={(elements, state, files) => {
            setReady(Boolean(reviewApi.current) && elements.every(element => element.type !== "image" || Boolean(element.fileId && files[element.fileId])));
            if (elements.length && !fitted.current && reviewApi.current) { fitted.current = true; reviewApi.current.scrollToContent(elements, { fitToViewport: true, maxZoom: 1 }); }
            setViewport([state.zoom.value, state.scrollX, state.scrollY, state.offsetLeft, state.offsetTop, state.width, state.height].join(","));
          }} />
          <svg ref={layer} role="application" aria-label={t("markups.areaLabel")} style={{ pointerEvents: navigating ? "none" : undefined }} className="absolute inset-0 z-20 h-full w-full touch-none cursor-crosshair" onPointerDown={event => {
            if (busy || !ready || event.button !== 0) return; event.preventDefault(); const point = scenePoint(event); if (!point) return;
            if (strokes.length >= 100) { setError(t("markups.tooMany")); return; }
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
            if (strokes.reduce((sum, item) => sum + item.points.length, 0) + stroke.points.length > 10000) { setError(t("markups.tooManyPoints")); return; }
            setStrokes(items => [...items, stroke]);
          }}><rect width="100%" height="100%" fill="transparent" />{[...strokes, ...(active ? [active] : [])].map(renderStroke)}</svg></> : selected ? <div className="flex h-full flex-col"><div className="shrink-0 space-y-2 border-b bg-background p-2 text-sm"><p>{showCurrent && currentReview?.markupId === selected.id ? t("markups.snapshotHeader", { current: currentReview.revision, marked: selected.at_revision }) : t("markups.previewHeader", { revision: selected.at_revision }) + (selected.at_revision !== getRevision() ? t("markups.historySuffix") : "")}</p>{showCurrent && currentReview && currentReview.revision !== getRevision() && <p role="status">{t("markups.snapshotStale")}</p>}{selected.at_revision !== getRevision() && <><div className="flex flex-wrap gap-2"><Button size="sm" variant={showCurrent ? "outline" : "secondary"} aria-pressed={!showCurrent} disabled={busy} onClick={() => setShowCurrent(false)}>{t("markups.viewHistory")}</Button><Button size="sm" variant={showCurrent ? "secondary" : "outline"} aria-pressed={showCurrent} disabled={busy} onClick={async () => {
              setBusy(true); setError(""); handoff.clearError(); setHistoryConfirmed(false);
              try {
                if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
                const api = getApi(), revision = getRevision(); if (!api || revision === undefined) throw new Error(t("markups.notReady"));
                const serialized = serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
                setCurrentReview({ markupId: selected.id, key: crypto.randomUUID(), revision, serialized, document: { ...JSON.parse(serialized), scrollToContent: true } }); setShowCurrent(true);
              } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
            }}>{t("markups.compareCurrent")}</Button></div><p className="text-xs text-muted-foreground">{t("markups.compareHint")}</p>{currentReview?.markupId === selected.id && currentReview.revision === getRevision() && <label className="flex items-center gap-2"><input type="checkbox" aria-label={t("markups.confirmedLabel")} disabled={busy} checked={historyConfirmed} onChange={event => setHistoryConfirmed(event.target.checked)} />{t("markups.confirmed", { revision: currentReview.revision })}</label>}</>}</div>
            {showCurrent && currentReview?.markupId === selected.id ? <div className="min-h-0 flex-1" role="region" aria-label={t("markups.regionLabel")}><CanvasPreview key={currentReview.key} document={currentReview.document} viewportZoomFactor={0.9} /></div> : <img alt={t("markups.savedAlt")} src={selected.dataURL} className="min-h-0 w-full flex-1 object-contain" />}</div> : <p className="p-4 text-sm text-slate-600">{t("markups.emptyState")}</p>}
      </section></div>
    </DialogContent>
  </Dialog>;
}
