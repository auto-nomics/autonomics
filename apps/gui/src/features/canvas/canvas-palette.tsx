import { CanvasIllustrationLabels, type IllustrationLabel } from "./canvas-illustration-labels";
import { assetGeometry } from "./canvas-asset-review";
import { CanvasIllustrationBounds } from "./canvas-illustration-bounds";
import { CanvasPartStyles } from "./canvas-part-styles";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Palette = { vector?: boolean; labels?: IllustrationLabel[]; image_geometry?: [number, number, number, number]; anchor_id?: string; visible_crop?: [number, number, number, number]; element_id: string; revision: number; file_id: string; colors: string[]; previewDataURL: string };
export function CanvasPalette({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied, capturePreview }: {
  capturePreview: () => Promise<boolean>; projectId: string; canvasId: string; disabled: boolean; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onApplied: (id: string) => void;
}) {
  const { t } = useTranslation("canvas");
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [palette, setPalette] = useState<Palette | null>(null), [colors, setColors] = useState<Record<string, string>>({});
  const [observed, setObserved] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><Button size="sm" variant="outline" disabled={disabled || busy} title={t("palette.buttonTitle")} onClick={async () => {
    setOpen(true); setBusy(true); setError(""); setPalette(null); setColors({});
    try {
      if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
      const api = getApi(), revision = getRevision(); if (!api || revision === undefined || isDirty()) throw new Error(t("palette.canvasChangedSaveFirst"));
      const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
      const anchors = elements.filter(element => selected[element.id]).map(element => element.customData?.visualAssetElementId);
      const images = elements.filter(element => !element.isDeleted && element.type === "image" && (selected[element.id] || anchors.includes(element.id)) && element.fileId && ["image/svg+xml", "image/png", "image/jpeg", "image/webp"].includes(api.getFiles()[element.fileId]?.mimeType ?? ""));
      if (images.length !== 1) throw new Error(t("palette.selectOneAsset"));
      const image = images[0];
      if (image.type !== "image" || !image.fileId) throw new Error(t("palette.fileMissing"));
      const fileId = image.fileId;
      const before = snapshot();
      const vector = api.getFiles()[fileId]?.mimeType === "image/svg+xml";
      const value: Palette = vector ? await gatewayRequest("GET", `${path}/vector-palette?element_id=${encodeURIComponent(images[0].id)}&expected_revision=${revision}`) as Palette : await gatewayRequest("GET", `${path}/image-info?element_id=${encodeURIComponent(images[0].id)}&expected_revision=${revision}`).then(value => { const info=value as {element_id:string;revision:number;dataURL:string}; return {element_id:info.element_id,revision:info.revision,file_id:fileId,previewDataURL:info.dataURL,colors:[]}; });
      if (snapshot() !== before || getRevision() !== revision || isDirty()) throw new Error(t("palette.changedDuringRead"));
      const anchor = elements.find(element => element.id === images[0].customData?.semanticAnchorId && element.customData?.visualAssetElementId === images[0].id);
      const labelIds = anchor?.customData?.visualLabelIds;
      const labels: IllustrationLabel[] = elements.filter(e => e.type === "text" && !e.isDeleted && !e.locked && e.angle === 0 && e.fontFamily === 9 && !e.containerId && Array.isArray(labelIds) && labelIds.includes(e.id)).map(e => { if (e.type !== "text") throw new Error(t("palette.labelTypeChanged")); return {id:e.id,text:e.originalText ?? e.text,x:e.x,y:e.y,width:e.width,fontSize:e.fontSize}; });
      const crop = assetGeometry(images[0] as unknown as Record<string, unknown>).fractions;
      setObserved(before); setPalette({ ...value, vector, labels, image_geometry: [images[0].x, images[0].y, images[0].width, images[0].height], anchor_id: anchor?.id, visible_crop: crop ? [crop[0], crop[1], crop[2], crop[3]] : undefined }); setColors(Object.fromEntries(value.colors.map(color => [color, color])));
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>{t("palette.button")}</Button><DialogContent showCloseButton={false} className="flex max-h-[85vh] w-[min(650px,95vw)] flex-col overflow-hidden"><DialogTitle>{palette?.vector === false ? t("palette.imageTitle") : t("palette.vectorTitle")}</DialogTitle>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {busy && !palette ? <p role="status">{t("palette.reading")}</p> : palette && <div className="min-h-0 overflow-auto"><p className="text-sm font-medium">{palette.vector ? t("palette.colorSection") : t("palette.layoutSection")}</p><img src={palette.previewDataURL} alt={t("palette.currentAsset")} className="my-2 max-h-64 w-full rounded bg-white object-contain" />{palette.vector ? <p className="mb-3 text-sm text-muted-foreground">{t("palette.vectorNote")}</p> : <p className="mb-3 text-sm text-muted-foreground">{t("palette.imageNote")}</p>}
      {palette.vector && !palette.colors.length && <p>{t("palette.noSolidColors")}</p>}
      <div className="grid grid-cols-2 gap-3">{palette.colors.map(color => <label key={color} className="flex items-center justify-between gap-3 rounded border p-2 text-sm"><span>{color}</span><input type="color" aria-label={t("palette.replaceColorLabel", { color })} disabled={busy} value={colors[color] ?? color} onChange={event => setColors(current => ({ ...current, [color]: event.target.value }))} /></label>)}</div>
      {palette.anchor_id && <CanvasIllustrationBounds key={`${palette.element_id}:${palette.revision}`} path={path} imageId={palette.element_id} fileId={palette.file_id} revision={palette.revision} preview={palette.previewDataURL} imageGeometry={palette.image_geometry} visibleCrop={palette.visible_crop} unchanged={() => !isDirty() && getRevision() === palette.revision && snapshot() === observed} busy={busy} onBusy={setBusy} onError={setError} onApplied={id => { setOpen(false); onApplied(id); }} />}
      {Boolean(palette.labels?.length) && <CanvasIllustrationLabels key={`labels:${palette.element_id}:${palette.revision}`} labels={palette.labels!} path={path} revision={palette.revision} unchanged={() => !isDirty() && getRevision() === palette.revision && snapshot() === observed} busy={busy} onBusy={setBusy} onError={setError} onApplied={id => { setOpen(false); onApplied(id); }} />}
      {palette.vector && <CanvasPartStyles capturePreview={capturePreview} key={palette.file_id} projectId={projectId} canvasId={canvasId} onPrepared={() => setOpen(false)} path={path} elementId={palette.element_id} revision={palette.revision} unchanged={() => !isDirty() && getRevision() === palette.revision && snapshot() === observed} busy={busy} onBusy={setBusy} onError={setError} onApplied={id => { setOpen(false); onApplied(id); }} />}
    </div>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>{t("palette.close")}</Button>{palette?.vector !== false && <Button size="sm" disabled={busy || !palette || !Object.entries(colors).some(([from, to]) => from !== to)} onClick={async () => {
      if (!palette || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== palette.revision || snapshot() !== observed) throw new Error(t("palette.canvasChangedReread"));
        const result = await gatewayRequest("POST", `${path}/vector-colors`, { element_id: palette.element_id, expected_revision: palette.revision, colors: Object.fromEntries(Object.entries(colors).filter(([from, to]) => from !== to)) }) as { id: string };
        if (isDirty() || getRevision() !== palette.revision || snapshot() !== observed) throw new Error(t("palette.canvasChangedDuringGeneration"));
        setOpen(false); onApplied(result.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("palette.apply")}</Button>}</div>
  </DialogContent></Dialog>;
}
