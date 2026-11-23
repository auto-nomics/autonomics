import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
export type PointEdit = { index: number; position: [number, number] };
type Handle = PointEdit & { kind: "anchor" | "control" };
type Geometry = { sha256: string; part_id: string; width: number; height: number; points: Handle[]; commands: { command: string; points: number[] }[]; previewDataURL: string };
export function CanvasPathPoints({ path, elementId, revision, sha256, partId, disabled, unchanged, onBusy, onError, onChange }: {
  path: string; elementId: string; revision: number; sha256: string; partId: string; disabled: boolean; unchanged: () => boolean;
  onBusy: (busy: boolean) => void; onError: (error: string) => void; onChange: (points: PointEdit[] | undefined) => void;
}) {
  const { t } = useTranslation("canvas");
  const [geometry, setGeometry] = useState<Geometry | null>(null), [selected, setSelected] = useState(0);
  const [changes, setChanges] = useState(new Map<number, [number, number]>());
  const current = useRef(changes);
  const drag = useRef<{ index: number; start: [number, number]; original: [number, number] } | null>(null);
  const position = (index: number): [number, number] => changes.get(index) ?? geometry!.points[index].position;
  const update = (index: number, point: [number, number]) => {
    if (!geometry) return;
    const next = new Map(current.current);
    if (point.every((value, axis) => Math.abs(value - geometry.points[index].position[axis]) < 1e-7)) next.delete(index);
    else { if (!next.has(index) && next.size >= 64) { onError(t("pathPoints.tooManyPoints")); return; } next.set(index, point); }
    current.current = next; setChanges(next); onChange(next.size ? [...next].map(([index, position]) => ({ index, position })) : undefined);
  };
  const root = (event: React.PointerEvent<SVGCircleElement>): [number, number] | null => {
    const matrix = event.currentTarget.ownerSVGElement?.getScreenCTM(); if (!matrix) return null;
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse()); return [point.x, point.y];
  };
  const reset = () => { const next = new Map<number, [number, number]>(); current.current = next; setChanges(next); onChange(undefined); };
  return <section aria-label={t("pathPoints.sectionLabel")} className="border-t pt-3">
    {!geometry ? <Button size="sm" variant="outline" disabled={disabled} onClick={async () => {
      onBusy(true); onError("");
      try {
        if (!unchanged()) throw new Error(t("pathPoints.canvasChanged"));
        const query = new URLSearchParams({ element_id: elementId, expected_revision: String(revision), sha256, part_id: partId });
        const value = await gatewayRequest("GET", `${path}/vector-geometry?${query}`) as Geometry;
        if (!unchanged()) throw new Error(t("pathPoints.changedDuringRead"));
        setGeometry(value); setSelected(0); reset();
      } catch (cause) { onError(String(cause)); } finally { onBusy(false); }
    }}>{t("pathPoints.load")}</Button> : <div className="flex flex-col gap-3">
      <p className="text-xs text-muted-foreground">{t("pathPoints.hint")}</p>
      <svg role="img" aria-label={t("pathPoints.previewLabel")} viewBox={`0 0 ${geometry.width} ${geometry.height}`} className="max-h-72 w-full rounded bg-white" style={{ touchAction: "none" }}>
        <image href={geometry.previewDataURL} width={geometry.width} height={geometry.height} />
        <path d={geometry.commands.map(command => `${command.command} ${command.points.map(index => position(index).join(" ")).join(" ")}`).join(" ")} fill="none" stroke="#f97316" strokeWidth={2} vectorEffect="non-scaling-stroke" pointerEvents="none" />
        {geometry.points.map(handle => { const p = position(handle.index); return <circle key={handle.index} cx={p[0]} cy={p[1]} r={Math.max(geometry.width, geometry.height) / (handle.index === selected ? 65 : 130)} fill={handle.kind === "control" ? "#8b5cf6" : "#f97316"} stroke="white" strokeWidth={1} vectorEffect="non-scaling-stroke" pointerEvents="none" />; })}
        <circle aria-label={t("pathPoints.dragLabel")} cx={position(selected)[0]} cy={position(selected)[1]} r={Math.max(geometry.width, geometry.height) / 65} fill="transparent" pointerEvents={disabled ? "none" : "all"} style={{ cursor: "move" }} onPointerDown={event => {
          if (disabled || event.button !== 0) return; const p = root(event); if (!p) return;
          event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); drag.current = { index: selected, start: p, original: [...position(selected)] };
        }} onPointerMove={event => {
          const gesture = drag.current; if (!gesture) return; const p = root(event); if (!p) return;
          update(gesture.index, [Math.max(-1e6, Math.min(1e6, gesture.original[0] + p[0] - gesture.start[0])), Math.max(-1e6, Math.min(1e6, gesture.original[1] + p[1] - gesture.start[1]))]);
        }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { const gesture = drag.current; drag.current = null; if (gesture) update(gesture.index, gesture.original); }} />
      </svg>
      <label className="text-sm">{t("pathPoints.point")}<select aria-label={t("pathPoints.pointLabel")} disabled={disabled} className="ml-2 rounded border bg-background p-1" value={selected} onChange={event => setSelected(Number(event.target.value))}>{geometry.points.map(handle => <option key={handle.index} value={handle.index}>{handle.index} · {handle.kind === "control" ? t("pathPoints.controlPoint") : t("pathPoints.anchorPoint")}</option>)}</select></label>
      <div className="flex flex-wrap gap-2">{([['X', 0], ['Y', 1]] as const).map(([axis, index]) => <label key={axis} className="text-sm">{axis}<input aria-label={t("pathPoints.axisLabel", { axis })} type="number" min={-1e6} max={1e6} step={1} disabled={disabled} value={position(selected)[index]} className="ml-1 w-32 rounded border bg-background p-1" onChange={event => { const next: [number, number] = [...position(selected)]; next[index] = Number(event.target.value); update(selected, next); }} /></label>)}</div>
      <Button size="sm" variant="outline" disabled={disabled || !changes.size} onClick={reset}>{t("pathPoints.reset")}</Button>
    </div>}
  </section>;
}
