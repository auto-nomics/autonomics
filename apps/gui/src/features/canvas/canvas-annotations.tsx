import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
export type Annotation = { id: string; element_ids: string[]; note: string; status: string; at_revision: number };
export function CanvasAnnotations({ projectId, canvasId, getApi, getRevision, ensureSaved, preparing = false, canPrepare = false, onPrepare, elementIds, active = true }: {
  projectId: string; canvasId: string; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>;
  elementIds?: readonly string[]; active?: boolean;
  preparing?: boolean; canPrepare?: boolean; onPrepare?: (annotation: Annotation) => Promise<boolean>;
}) {
  const { t } = useTranslation("canvas");
  const [notes, setNotes] = useState<Annotation[]>([]);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/annotations`;
  useEffect(() => { if (!active) return; let cancelled = false; void gatewayRequest("GET", path).then(items => { if (!cancelled) setNotes(items as Annotation[]); }).catch(cause => { if (!cancelled) setError(String(cause)); }); return () => { cancelled = true; }; }, [path, active]);
  const save = async (prepare: boolean) => {
      const api = getApi(); if (!api || busy || preparing) return;
      const targets = elementIds ? [...elementIds] : Object.keys(api.getAppState().selectedElementIds).filter(id => api.getAppState().selectedElementIds[id]);
      if (!targets.length) { setError(t("annotations.selectFirst")); return; }
      setBusy(true); setError("");
      try {
        if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
        if (targets.some(id => !api.getSceneElements().some(element => element.id === id && !element.isDeleted))) throw new Error(t("annotations.targetsChanged"));
        const added = await gatewayRequest("POST", path, { expected_revision: getRevision(), element_ids: targets, note: note.trim() }) as Annotation;
        setNotes(items => [...items, added]); setNote("");
        if (prepare && onPrepare && !await onPrepare(added)) setError(t("annotations.savedDraftFailed"));
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }

  };
  return <section aria-label={t("annotations.title")} className="canvas-annotations flex flex-col gap-2">
    <p className="text-xs text-muted-foreground">{t("annotations.hint")}</p>
    <textarea aria-label={t("annotations.noteLabel")} disabled={busy || preparing} maxLength={4000} className="min-h-20 rounded border bg-background p-2" value={note} onChange={event => setNote(event.target.value)} />
    <Button size="sm" disabled={busy || preparing || !note.trim()} onClick={() => void save(false)}>{t("annotations.annotateSelection")}</Button>
    {onPrepare && <><Button size="sm" disabled={busy || preparing || !canPrepare || !note.trim()} onClick={() => void save(true)}>{t("annotations.annotateAndPrepare")}</Button><p className="text-xs text-muted-foreground">{t("annotations.prepareHint")}</p></>}
    {error && <p role="alert" className="text-destructive">{error}</p>}
    {notes.filter(item => !elementIds || item.element_ids.some(id => elementIds.includes(id))).map(item => <article key={item.id} className="rounded border p-2"><p className="whitespace-pre-wrap">{item.note}</p><p className="mt-1 text-xs text-muted-foreground">v{item.at_revision} · {item.status === "resolved" ? t("markups.statusResolved") : t("markups.statusOpen")}</p><div className="mt-2 flex flex-wrap gap-1"><Button size="sm" variant="outline" disabled={busy || preparing} onClick={() => {
      const api = getApi(); if (!api) return; const elements = api.getSceneElements().filter(element => item.element_ids.includes(element.id));
      if (!elements.length) { setError(t("annotations.objectsGone")); return; }
      api.updateScene({ appState: { selectedElementIds: Object.fromEntries(elements.map(element => [element.id, true])) } }); api.scrollToContent(elements);
    }}>{t("annotations.locateObjects")}</Button>{item.status === "open" && onPrepare && <Button size="sm" variant="outline" disabled={busy || preparing || !canPrepare} onClick={async () => { setBusy(true); setError(""); try { if (!await onPrepare(item)) setError(t("annotations.draftFailed")); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>{t("annotations.prepareWithThis")}</Button>}{item.status !== "resolved" && <Button size="sm" variant="outline" disabled={busy || preparing} onClick={async () => {
      setBusy(true); setError(""); try { await gatewayRequest("POST", `${path}/${encodeURIComponent(item.id)}/resolve`); setNotes(items => items.map(current => current.id === item.id ? { ...current, status: "resolved" } : current)); } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("annotations.markResolved")}</Button>}</div></article>)}
  </section>;
}
