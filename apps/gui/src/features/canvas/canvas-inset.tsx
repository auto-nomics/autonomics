import { useId, useRef, useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { CanvasInsetRefresh } from "./canvas-inset-refresh";
import { imagePlacement } from "./canvas-image-placement";
type Region = { x: number; y: number; width: number; height: number };
type Info = { element_id: string; revision: number; natural_width: number; natural_height: number; visible_crop: number[]; angle: number; scale: number[]; dataURL: string; displayWidth: number; displayHeight: number };
export function CanvasInset({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: {
  projectId: string; canvasId: string; disabled: boolean; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onProposed: (id: string) => void;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [info, setInfo] = useState<Info | null>(null), [observed, setObserved] = useState("");
  const [region, setRegion] = useState<Region>({ x: .25, y: .25, width: .5, height: .5 }), [title, setTitle] = useState("局部放大");
  const previewClip = `inset-preview-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const imageViewport = useRef<SVGGElement | null>(null);
  const previewBounds = info ? { width: Math.abs(Math.cos(info.angle)) * info.displayWidth + Math.abs(Math.sin(info.angle)) * info.displayHeight, height: Math.abs(Math.sin(info.angle)) * info.displayWidth + Math.abs(Math.cos(info.angle)) * info.displayHeight } : { width: 1, height: 1 };
  const drag = useRef<{ x: number; y: number } | null>(null);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  const unchanged = () => info && !isDirty() && getRevision() === info.revision && snapshot() === observed;
  return <><CanvasInsetRefresh {...{ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }} /><Button size="sm" variant="outline" disabled={disabled || busy} title="选中放大图内容组或边框，预览删除图组、区域框和引导线" onClick={async () => {
    setBusy(true); setError("");
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请保存后重试。");
      const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
      const anchors = elements.filter(element => !element.isDeleted && element.customData?.inset?.kind === "image_inset_snapshot" && element.customData?.inset?.anchor_id === element.id && (selected[element.id] || elements.some(child => selected[child.id] && child.id === element.customData?.visualAssetElementId && child.customData?.semanticAnchorId === element.id)));
      if (anchors.length !== 1) throw new Error("请只选中一个放大图的内容组或边框。");
      const before = snapshot();
      const proposal = await gatewayRequest("POST", `${path}/proposals`, { expected_revision: revision, summary: "删除放大图及其区域注释", operations: [{ op: "remove", id: anchors[0].id }] }) as { id: string };
      if (isDirty() || getRevision() !== revision || snapshot() !== before) throw new Error("生成期间画布发生变化；提案已保留，请在提案列表核对。");
      onProposed(proposal.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>删除放大图</Button>{!open && error && <span role="alert" className="text-xs text-destructive">{error}</span>}<Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setInfo(null); setBusy(true); setError(""); setTitle("局部放大"); setRegion({ x: .25, y: .25, width: .5, height: .5 });
      try {
        if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请保存后重试。");
        const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
        const anchors = elements.filter(element => selected[element.id]).map(element => element.customData?.visualAssetElementId);
        const images = elements.filter(element => !element.isDeleted && element.type === "image" && (selected[element.id] || anchors.includes(element.id)));
        if (images.length !== 1) throw new Error("请只选中一张图片或其语义锚点。");
        const before = snapshot();
        const value = await gatewayRequest("GET", `${path}/image-info?element_id=${encodeURIComponent(images[0].id)}&expected_revision=${revision}`) as Info;
        if (isDirty() || getRevision() !== revision || snapshot() !== before) throw new Error("读取期间画布发生变化，请重新打开。");
        if (!Number.isFinite(value.angle) || !value.scale.every(v => v === 1 || v === -1) || value.scale.length !== 2 || images[0].width <= 0 || images[0].height <= 0) throw new Error("图片变换无效，请先恢复图片的几何参数。");
        setObserved(before); setInfo({ ...value, displayWidth: images[0].width, displayHeight: images[0].height });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>局部放大</Button>
    <DialogContent showCloseButton={false} className="flex max-h-[90vh] w-[min(650px,95vw)] flex-col overflow-auto"><DialogTitle>创建局部放大</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {busy && !info && <p role="status">读取图片…</p>}
      {info && <>
        <p className="text-sm text-muted-foreground">沿图片方向拖动框选区域，旋转、镜像和已有裁剪与画布保持一致。百分比坐标以图片自身方向为准。</p>
        <svg role="img" aria-label="框选放大区域" viewBox={`0 0 ${previewBounds.width} ${previewBounds.height}`} className="h-72 w-full touch-none select-none rounded border bg-white" style={{ cursor: busy ? "wait" : "crosshair" }}
          onPointerDown={event => {
            if (busy) return; const matrix = imageViewport.current?.getScreenCTM(); if (!matrix) return;
            const p = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
            if (p.x < 0 || p.y < 0 || p.x > info.displayWidth || p.y > info.displayHeight) return;
            event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
            drag.current = { x: p.x / info.displayWidth, y: p.y / info.displayHeight };
          }} onPointerMove={event => {
            if (!drag.current || busy) return; const matrix = imageViewport.current?.getScreenCTM(); if (!matrix) return;
            const p = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
            const x = Math.max(0, Math.min(1, p.x / info.displayWidth)), y = Math.max(0, Math.min(1, p.y / info.displayHeight));
            setRegion({ x: Math.min(x, drag.current.x), y: Math.min(y, drag.current.y), width: Math.abs(x - drag.current.x), height: Math.abs(y - drag.current.y) });
          }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} onLostPointerCapture={() => { drag.current = null; }}>
          <defs><clipPath id={previewClip}><rect width={info.displayWidth} height={info.displayHeight} /></clipPath></defs>
          <g ref={imageViewport} data-inset-image-viewport transform={`translate(${previewBounds.width / 2} ${previewBounds.height / 2}) rotate(${info.angle * 180 / Math.PI}) translate(${-info.displayWidth / 2} ${-info.displayHeight / 2})`}>
          <g clipPath={`url(#${previewClip})`}><g transform={`translate(${info.scale[0] < 0 ? info.displayWidth : 0} ${info.scale[1] < 0 ? info.displayHeight : 0}) scale(${info.scale[0]} ${info.scale[1]})`}>
            <image href={info.dataURL} x={-info.visible_crop[0] / info.visible_crop[2] * info.displayWidth} y={-info.visible_crop[1] / info.visible_crop[3] * info.displayHeight} width={info.natural_width / info.visible_crop[2] * info.displayWidth} height={info.natural_height / info.visible_crop[3] * info.displayHeight} preserveAspectRatio="none" />
          </g></g>
          <rect x={region.x * info.displayWidth} y={region.y * info.displayHeight} width={region.width * info.displayWidth} height={region.height * info.displayHeight} fill="#3b82f626" stroke="#2563eb" strokeWidth="2" vectorEffect="non-scaling-stroke" pointerEvents="none" />
          </g>
        </svg>
        <div className="grid grid-cols-2 gap-2">{(["x", "y", "width", "height"] as const).map((key, index) => <label key={key} className="text-sm">{["左侧", "顶部", "宽度", "高度"][index]} (%)<input aria-label={`放大区域 ${key}`} type="number" min="0" max="100" step="0.1" disabled={busy} className="ml-2 w-24 rounded border p-1" value={Number((region[key] * 100).toFixed(3))} onChange={event => setRegion(current => ({ ...current, [key]: Number(event.target.value) / 100 }))} /></label>)}</div>
        <label className="text-sm">标题<input aria-label="放大图标题" value={title} maxLength={80} disabled={busy} className="ml-2 rounded border p-1" onChange={event => setTitle(event.target.value)} /></label>
        <p className="text-sm text-muted-foreground">保留原始文件和来源，生成带区域框及虚线引导的固定快照。以后修改或移动原图，不会自动刷新快照或区域框。</p>
      </>}
      <div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>取消放大</Button><Button size="sm" disabled={busy || !info || !title.trim() || region.width <= 0 || region.height <= 0 || region.x < 0 || region.y < 0 || region.x + region.width > 1.000000001 || region.y + region.height > 1.000000001} onClick={async () => {
        if (!info || busy) return; setBusy(true); setError("");
        try {
          if (!unchanged()) throw new Error("画布发生变化，请取消并重新框选。");
          const api = getApi(); if (!api) throw new Error("画布不可用。");
          const placement = imagePlacement(api, 344, 440);
          const proposal = await gatewayRequest("POST", `${path}/insets`, { expected_revision: info.revision, inset: { element_id: info.element_id, region, x: placement.x, y: placement.y, max_size: 320, title } }) as { id: string };
          if (!unchanged()) throw new Error("生成期间画布发生变化；提案已保留，请在提案列表核对。");
          setOpen(false); onProposed(proposal.id);
        } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>预览放大提案</Button></div>
    </DialogContent>
  </Dialog></>;
}
