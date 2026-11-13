import { useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";

type Bounds = [number, number, number, number];
export function CanvasIllustrationBounds({ path, imageId, fileId, revision, preview, visibleCrop, imageGeometry, unchanged, busy, onBusy, onError, onProposed }: {
  path: string; imageId: string; fileId: string; revision: number; preview: string; visibleCrop?: Bounds; imageGeometry?: Bounds;
  unchanged: () => boolean; busy: boolean; onBusy: (value: boolean) => void;
  onError: (message: string) => void; onProposed: (id: string) => void;
}) {
  const [dimensions, setDimensions] = useState<[number, number] | null>(null);
  const [editGeometry, setEditGeometry] = useState(false);
  const [geometry, setGeometry] = useState<Bounds>(imageGeometry ?? [0, 0, 100, 100]);
  const validGeometry = !editGeometry || (geometry.every(value => Number.isFinite(value) && Math.abs(value) <= 1e6) && geometry[2] > 0 && geometry[3] > 0);
  const [bounds, setBounds] = useState<Bounds | null>(null);
  const gesture = useRef<{ start: [number, number]; previous: Bounds | null } | null>(null);
  const point = (event: React.PointerEvent<SVGSVGElement>): [number, number] | null => {
    const matrix = event.currentTarget.getScreenCTM(); if (!matrix || !dimensions) return null;
    const p = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
    return [Math.max(0, Math.min(1, p.x / dimensions[0])), Math.max(0, Math.min(1, p.y / dimensions[1]))];
  };
  const crop = visibleCrop ?? [0, 0, 1, 1];
  const valid = bounds !== null && bounds.every(Number.isFinite) && bounds[0] >= 0 && bounds[1] >= 0 && bounds[2] > 0 && bounds[3] > 0 && bounds[0] + bounds[2] <= 1 && bounds[1] + bounds[3] <= 1 && bounds[0] >= crop[0] - 1e-9 && bounds[1] >= crop[1] - 1e-9 && bounds[0] + bounds[2] <= crop[0] + crop[2] + 1e-9 && bounds[1] + bounds[3] <= crop[1] + crop[3] + 1e-9;
  return <section aria-label="插图贴合范围" className="mt-4 border-t pt-3">
    <p className="text-sm font-medium">插图贴合范围</p>
    <p className="my-2 text-xs text-muted-foreground">在原始素材中拖框，选中连接线应靠近的可见内容。只调整该插图的语义锚点及相连线端点；图片、裁剪和科学依据保留。旋转、镜像按画布变换计算，锚点使用可见范围的轴对齐外框。范围不代表生物学结构，采用前请核对预览。</p>
    {visibleCrop && <p className="mb-2 text-xs text-muted-foreground">灰色区域已被画布裁剪，框选须位于虚线标示的可见区域内。</p>}
    <img src={preview} alt="" className="hidden" onLoad={event => { const image = event.currentTarget; if (image.naturalWidth > 0 && image.naturalHeight > 0) setDimensions([image.naturalWidth, image.naturalHeight]); }} />
    {dimensions && <svg role="img" aria-label="框选插图贴合范围" viewBox={`0 0 ${dimensions[0]} ${dimensions[1]}`} className="max-h-72 w-full rounded border bg-white" style={{ touchAction: "none", cursor: busy ? undefined : "crosshair" }} onPointerDown={event => {
      if (busy || event.button !== 0) return; const p = point(event); if (!p) return;
      event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); gesture.current = { start: p, previous: bounds }; setBounds([p[0], p[1], 0, 0]);
    }} onPointerMove={event => {
      if (!gesture.current || busy) return; const p = point(event); if (!p) return;
      const start = gesture.current.start; const x = Math.min(start[0], p[0]), y = Math.min(start[1], p[1]);
      setBounds([x, y, Math.min(Math.abs(start[0] - p[0]), 1 - x), Math.min(Math.abs(start[1] - p[1]), 1 - y)]);
    }} onPointerUp={event => { if (gesture.current) { gesture.current = null; event.currentTarget.releasePointerCapture(event.pointerId); } }} onPointerCancel={() => { const prior = gesture.current; gesture.current = null; if (prior) setBounds(prior.previous); }}>
      <image href={preview} width={dimensions[0]} height={dimensions[1]} />
      {visibleCrop && <><path d={`M0 0H${dimensions[0]}V${dimensions[1]}H0Z M${crop[0] * dimensions[0]} ${crop[1] * dimensions[1]}h${crop[2] * dimensions[0]}v${crop[3] * dimensions[1]}h${-crop[2] * dimensions[0]}Z`} fill="#64748b80" fillRule="evenodd" /><rect x={crop[0] * dimensions[0]} y={crop[1] * dimensions[1]} width={crop[2] * dimensions[0]} height={crop[3] * dimensions[1]} fill="none" stroke="#2563eb" strokeDasharray="4 3" vectorEffect="non-scaling-stroke" /></>}
      {valid && bounds && <rect x={bounds[0] * dimensions[0]} y={bounds[1] * dimensions[1]} width={bounds[2] * dimensions[0]} height={bounds[3] * dimensions[1]} fill="#f9731620" stroke="#f97316" strokeWidth={2} vectorEffect="non-scaling-stroke" />}
    </svg>}
    <div className="my-2 flex flex-wrap gap-2">{(["左侧", "顶部", "宽度", "高度"] as const).map((label, index) => <label key={label} className="text-xs">{label} %<input aria-label={`贴合范围${label}百分比`} type="number" min={0} max={100} step={0.1} disabled={busy} value={bounds && Number.isFinite(bounds[index]) ? Number((bounds[index] * 100).toFixed(6)) : ""} className="ml-1 w-20 rounded border bg-background p-1" onChange={event => { const value = event.currentTarget.valueAsNumber / 100; setBounds(current => { const next: Bounds = [...(current ?? [0, 0, 1, 1])]; next[index] = value; return next; }); }} /></label>)}</div>
    {bounds && !valid && <p role="status" className="mb-2 text-xs text-destructive">请选择当前可见区域内的非空范围；左侧加宽度、顶部加高度均不能超过 100%。</p>}
    {imageGeometry && <div className="my-3 space-y-2 text-xs"><label className="flex items-center gap-2"><input type="checkbox" aria-label="调整插图位置和尺寸" disabled={busy} checked={editGeometry} onChange={event => setEditGeometry(event.target.checked)} />同时调整插图位置和尺寸</label>{editGeometry && <><p>位置使用画布坐标，尺寸使用画布单位。保留裁剪、旋转、镜像和来源，并按上方范围重新连接；标签位置单独保留，请在候选中核对。旋转后的完整图片须位于所属面板内。</p><div className="flex flex-wrap gap-2">{(["X", "Y", "宽度", "高度"] as const).map((name,index) => <label key={name}>{name}<input aria-label={`插图几何${name}`} type="number" disabled={busy} value={Number.isFinite(geometry[index]) ? geometry[index] : ""} className="ml-1 w-24 rounded border bg-background p-1" onChange={event => { const value=event.currentTarget.valueAsNumber; setGeometry(current => { const next: Bounds=[...current]; next[index]=value; return next; }); }} /></label>)}</div>{!validGeometry && <p role="status" className="text-destructive">位置须为有效数字，宽度和高度须大于零，各项绝对值不能超过 1000000。</p>}</>}</div>}
    <div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setBounds([...crop] as Bounds)}>使用当前可见范围</Button><Button size="sm" disabled={busy || !valid || !validGeometry} onClick={async () => {
      if (!valid || !bounds || !validGeometry) return; onBusy(true); onError("");
      try {
        if (!unchanged()) throw new Error("画布发生变化，请重新打开素材编辑并选择范围。");
        const result = await gatewayRequest("POST", `${path}/proposals`, { expected_revision: revision, summary: editGeometry ? "调整插图布局与连接范围" : "调整插图连接范围", operations: [editGeometry ? { op: "set_illustration_geometry", id: imageId, file_id: fileId, x: geometry[0], y: geometry[1], width: geometry[2], height: geometry[3], normalized_bounds: bounds } : { op: "fit_illustration_anchor", id: imageId, file_id: fileId, normalized_bounds: bounds }] }) as { id: string };
        if (!unchanged()) throw new Error("生成提案期间画布发生变化。提案已保留，请保存后在提案列表核对版本。");
        onProposed(result.id);
      } catch (cause) { onError(String(cause)); } finally { onBusy(false); }
    }}>预览贴合提案</Button></div>
  </section>;
}
