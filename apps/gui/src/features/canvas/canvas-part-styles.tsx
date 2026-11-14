import { CanvasPathAgent } from "./canvas-path-agent";
import { CanvasPathPoints, type PointEdit } from "./canvas-path-points";
import { useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
type Part = { part_id: string; index: number; svg_id: string | null; fill: string; stroke: string; stroke_width: number; opacity: number; bounds: [number, number, number, number]; translation_supported: boolean; geometry_supported: boolean; point_count: number; changed_preview_pixels_when_hidden?: number };
type Inspection = { sha256: string; width: number; height: number; parts: Part[]; previewDataURL: string; render_contribution?: { status: "completed"; width: number; height: number } | { status: "not_computed"; reason: string } };
type Edit = { part_id: string; fill?: string; stroke?: string; stroke_width?: number; opacity?: number; translate?: [number, number]; points?: PointEdit[] };
export function CanvasPartStyles({ projectId, canvasId, path, elementId, revision, unchanged, onApplied, onPrepared, onError, busy, onBusy, capturePreview }: {
  capturePreview: () => Promise<boolean>; projectId: string; canvasId: string; onPrepared: () => void;
  path: string; elementId: string; revision: number; unchanged: () => boolean; onApplied: (id: string) => void;
  onError: (error: string) => void; busy: boolean; onBusy: (value: boolean) => void;
}) {
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [selected, setSelected] = useState(0), [edit, setEdit] = useState<Partial<Edit>>({});
  const part = inspection?.parts[selected];
  const drag = useRef<{ start: [number, number]; delta: [number, number] } | null>(null);
  const point = (event: React.PointerEvent<SVGGElement>): [number, number] | null => {
    const matrix = event.currentTarget.ownerSVGElement?.getScreenCTM(); if (!matrix) return null;
    const p = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse()); return [p.x, p.y];
  };
  const delta = edit.translate ?? [0, 0];
  const check = () => { if (!unchanged()) throw new Error("画布发生变化，请重新打开素材配色并读取路径。"); };
  return <section aria-label="素材内部路径" className="mt-4 border-t pt-3">
    <Button size="sm" variant="outline" disabled={busy} onClick={async () => {
      onBusy(true); onError("");
      try { check(); const value = await gatewayRequest("GET", `${path}/vector-parts?element_id=${encodeURIComponent(elementId)}&expected_revision=${revision}`) as Inspection; check(); setInspection(value); setSelected(0); setEdit({}); }
      catch (cause) { onError(String(cause)); } finally { onBusy(false); }
    }}>读取内部路径</Button>
    {inspection && part && <div className="mt-3 flex flex-col gap-3">
      <p className="text-xs text-muted-foreground">编号仅标识当前文件中的图形路径，不代表生物学组件。可修改所选路径样式或在源 SVG 坐标中平移。拖动橙框中心圆点可设置平移，预览修改核对实际效果。上层裁剪和素材边界保持原位，超出部分会被裁掉。透明度系数相对当前效果叠加，1 保持、0 隐藏；隐藏后可通过历史版本恢复。下方可继续读取并编辑端点与曲线控制点。</p>
      {inspection.render_contribution?.status === "completed" && typeof part.changed_preview_pixels_when_hidden === "number" && <p aria-label="路径渲染贡献" className="text-xs text-muted-foreground">在 {inspection.render_contribution.width} × {inspection.render_contribution.height} 的透明背景 SVG 预览中，隐藏此路径会改变 {part.changed_preview_pixels_when_hidden} 个像素。这不是路径面积；0 可能来自遮挡、同色覆盖、裁剪或细小结构。请结合预览核对可见效果。</p>}
      {inspection.render_contribution?.status === "not_computed" && <p aria-label="路径渲染贡献" className="text-xs text-muted-foreground">此素材超过逐路径比较的计算上限，未计算渲染贡献。请以预览为准。</p>}
      <svg role="img" aria-label={`选中的素材路径 ${part.index}`} viewBox={`0 0 ${inspection.width} ${inspection.height}`} className="max-h-64 w-full rounded bg-white">
        <image href={inspection.previewDataURL} width={inspection.width} height={inspection.height} />
        <g transform={`translate(${delta[0]} ${delta[1]})`} style={{ cursor: !busy && part.translation_supported ? "move" : undefined, touchAction: "none" }} onPointerDown={event => {
          if (busy || !part.translation_supported || Boolean(edit.points?.length) || event.button !== 0) return; const p = point(event); if (!p) return;
          event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); drag.current = { start: p, delta: [...delta] }; setEdit(current => ({ ...current, translate: [...delta] }));
        }} onPointerMove={event => {
          if (!drag.current) return; const p = point(event); if (!p) return;
          const gesture = drag.current;
          const next: [number, number] = [Math.max(-100000, Math.min(100000, gesture.delta[0] + p[0] - gesture.start[0])), Math.max(-100000, Math.min(100000, gesture.delta[1] + p[1] - gesture.start[1]))];
          setEdit(current => ({ ...current, translate: next }));
        }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { const previous = drag.current; drag.current = null; if (previous) setEdit(current => ({ ...current, translate: previous.delta })); }}>
          <rect x={part.bounds[0]} y={part.bounds[1]} width={part.bounds[2]} height={part.bounds[3]} fill="transparent" stroke="#f97316" vectorEffect="non-scaling-stroke" strokeWidth={2} />
          {part.translation_supported && !edit.points?.length && <circle aria-label="拖动路径平移" cx={part.bounds[0] + part.bounds[2] / 2} cy={part.bounds[1] + part.bounds[3] / 2} r={Math.max(inspection.width, inspection.height) / 80} fill="#f97316" stroke="white" vectorEffect="non-scaling-stroke" strokeWidth={1} />}
        </g>
      </svg>
      <label className="text-sm">路径<select aria-label="素材路径" disabled={busy} value={selected} className="ml-2 max-w-full rounded border bg-background p-1" onChange={event => { setSelected(Number(event.target.value)); setEdit({}); }}>{inspection.parts.map((p, index) => <option key={p.part_id} value={index}>{p.index} · {p.svg_id || "未命名路径"}</option>)}</select></label>
      {([['fill', '填充'], ['stroke', '描边']] as const).map(([key, label]) => <div key={key} className="flex items-center gap-3 text-sm">
        <label className="flex items-center gap-2"><input type="checkbox" aria-label={`修改路径${label}`} disabled={busy} checked={edit[key] !== undefined} onChange={event => setEdit(current => ({ ...current, [key]: event.target.checked ? /^#[a-f\d]{6}$/i.test(part[key]) || part[key] === "none" ? part[key] : "#000000" : undefined }))} />{label}</label>
        <span className="max-w-40 truncate text-xs text-muted-foreground" title={part[key]}>当前 {part[key]}</span>
        {edit[key] !== undefined && <><label><input type="checkbox" aria-label={`路径${label}设为空`} disabled={busy} checked={edit[key] === "none"} onChange={event => setEdit(current => ({ ...current, [key]: event.target.checked ? "none" : "#000000" }))} /> 无</label><input aria-label={`路径${label}颜色`} type="color" disabled={busy || edit[key] === "none"} value={edit[key] === "none" ? "#000000" : edit[key]} onChange={event => setEdit(current => ({ ...current, [key]: event.target.value }))} /></>}
      </div>)}
      <div className="flex items-center gap-3 text-sm"><label><input type="checkbox" aria-label="修改路径描边宽度" disabled={busy} checked={edit.stroke_width!==undefined} onChange={event=>setEdit(current=>({...current,stroke_width:event.target.checked?(part.stroke_width||1):undefined}))}/> 描边宽度</label><span>当前 {part.stroke_width} SVG 单位</span>{edit.stroke_width!==undefined&&<input aria-label="路径描边宽度" type="number" min={0.001} max={1000} step="any" value={edit.stroke_width} disabled={busy} onChange={event=>setEdit(current=>({...current,stroke_width:Number(event.target.value)}))} className="w-24 rounded border bg-background p-1"/>}</div>
      <p className="text-xs text-muted-foreground">描边宽度按 SVG 局部坐标设置，画布缩放和路径变换会影响显示粗细；无描边的路径还需显式设置描边颜色。加粗可能扩大可见范围或被原裁剪截断，请查看预览。</p>
      <div className="flex items-center gap-3 text-sm"><label className="flex items-center gap-2"><input type="checkbox" aria-label="修改路径透明度" disabled={busy} checked={edit.opacity !== undefined} onChange={event => setEdit(current => ({ ...current, opacity: event.target.checked ? part.opacity : undefined }))} />透明度系数</label>{edit.opacity !== undefined && <input aria-label="路径透明度" type="number" min={0} max={1} step={0.05} value={edit.opacity} disabled={busy} className="w-24 rounded border bg-background p-1" onChange={event => setEdit(current => ({ ...current, opacity: Number(event.target.value) }))} />}</div>
      <div className="flex flex-wrap items-center gap-2 text-sm"><label className="flex items-center gap-2"><input type="checkbox" aria-label="平移路径" disabled={busy || !part.translation_supported || Boolean(edit.points?.length)} checked={edit.translate !== undefined} onChange={event => setEdit(current => ({ ...current, translate: event.target.checked ? [0, 0] : undefined }))} />平移路径</label>{edit.translate && ([['X', 0], ['Y', 1]] as const).map(([axis, index]) => <label key={axis}>{axis}<input aria-label={`路径平移 ${axis}`} type="number" min={-100000} max={100000} step={1} disabled={busy} className="ml-1 w-24 rounded border bg-background p-1" value={edit.translate![index]} onChange={event => setEdit(current => { const next: [number, number] = [...(current.translate ?? [0, 0])]; next[index] = Number(event.target.value); return { ...current, translate: next }; })} /></label>)}</div>
      {part.geometry_supported ? <CanvasPathPoints key={part.part_id} path={path} elementId={elementId} revision={revision} sha256={inspection.sha256} partId={part.part_id} disabled={busy || edit.translate !== undefined} unchanged={unchanged} onBusy={onBusy} onError={onError} onChange={points => setEdit(current => ({ ...current, points }))} /> : <p className="text-xs text-muted-foreground">此路径的变换不可编辑，或包含超过 512 个点。</p>}
      <CanvasPathAgent capturePreview={capturePreview} key={part.part_id} projectId={projectId} canvasId={canvasId} elementId={elementId} revision={revision} sha256={inspection.sha256} partId={part.part_id} partIndex={part.index} busy={busy} unchanged={unchanged} onBusy={onBusy} onError={onError} onPrepared={onPrepared} />
      <Button size="sm" disabled={busy || !Object.values(edit).some(value => value !== undefined) || (edit.stroke_width!==undefined&&(!Number.isFinite(edit.stroke_width)||edit.stroke_width<=0||edit.stroke_width>1000))} onClick={async () => {
        onBusy(true); onError("");
        try { check(); const result = await gatewayRequest("POST", `${path}/vector-parts`, { element_id: elementId, expected_revision: revision, sha256: inspection.sha256, edits: [{ ...edit, part_id: part.part_id }] }) as { id: string }; check(); onApplied(result.id); }
        catch (cause) { onError(String(cause)); } finally { onBusy(false); }
      }}>应用路径修改</Button>
    </div>}
  </section>;
}
