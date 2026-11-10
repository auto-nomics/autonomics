import { CanvasPartStyles } from "./canvas-part-styles";
import { useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Palette = { element_id: string; revision: number; file_id: string; colors: string[]; previewDataURL: string };
export function CanvasPalette({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: {
  projectId: string; canvasId: string; disabled: boolean; getApi: () => ExcalidrawImperativeAPI | null;
  getRevision: () => number | undefined; ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onProposed: (id: string) => void;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [palette, setPalette] = useState<Palette | null>(null), [colors, setColors] = useState<Record<string, string>>({});
  const [observed, setObserved] = useState("");
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><Button size="sm" variant="outline" disabled={disabled || busy} title="选中一个 SVG 素材，编辑整体配色或内部路径；PNG 不支持内部路径编辑" onClick={async () => {
    setOpen(true); setBusy(true); setError(""); setPalette(null); setColors({});
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision(); if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请先保存后重试。");
      const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
      const anchors = elements.filter(element => selected[element.id]).map(element => element.customData?.visualAssetElementId);
      const images = elements.filter(element => !element.isDeleted && element.type === "image" && (selected[element.id] || anchors.includes(element.id)) && element.fileId && api.getFiles()[element.fileId]?.mimeType === "image/svg+xml");
      if (images.length !== 1) throw new Error("请只选中一个 SVG 素材（或其语义锚点）再调整配色。");
      const before = snapshot();
      const value = await gatewayRequest("GET", `${path}/vector-palette?element_id=${encodeURIComponent(images[0].id)}&expected_revision=${revision}`) as Palette;
      if (snapshot() !== before || getRevision() !== revision || isDirty()) throw new Error("读取配色期间画布发生变化，请重新打开配色面板。");
      setObserved(before); setPalette(value); setColors(Object.fromEntries(value.colors.map(color => [color, color])));
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>素材编辑</Button><DialogContent showCloseButton={false} className="flex max-h-[85vh] w-[min(650px,95vw)] flex-col overflow-hidden"><DialogTitle>矢量素材编辑</DialogTitle>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {busy && !palette ? <p role="status">读取素材配色…</p> : palette && <div className="min-h-0 overflow-auto"><p className="text-sm font-medium">整体配色</p><img src={palette.previewDataURL} alt="配色前的矢量素材" className="my-2 max-h-64 w-full rounded bg-white object-contain" /><p className="mb-3 text-sm text-muted-foreground">相同颜色的填充、描边和渐变色将一起替换，每次最多 32 种；SVG 内的文字须已转为路径。只修改当前素材实例，不改变其他实例、文字标记形状或布局。修改后先预览提案。</p>
      {!palette.colors.length && <p>没有可替换的实色。</p>}
      <div className="grid grid-cols-2 gap-3">{palette.colors.map(color => <label key={color} className="flex items-center justify-between gap-3 rounded border p-2 text-sm"><span>{color}</span><input type="color" aria-label={`替换颜色 ${color}`} disabled={busy} value={colors[color] ?? color} onChange={event => setColors(current => ({ ...current, [color]: event.target.value }))} /></label>)}</div>
      <CanvasPartStyles key={palette.file_id} path={path} elementId={palette.element_id} revision={palette.revision} unchanged={() => !isDirty() && getRevision() === palette.revision && snapshot() === observed} busy={busy} onBusy={setBusy} onError={setError} onProposed={id => { setOpen(false); onProposed(id); }} />
    </div>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>关闭素材编辑</Button><Button size="sm" disabled={busy || !palette || !Object.entries(colors).some(([from, to]) => from !== to)} onClick={async () => {
      if (!palette || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== palette.revision || snapshot() !== observed) throw new Error("画布发生变化，请取消后重新读取素材配色。");
        const result = await gatewayRequest("POST", `${path}/vector-colors`, { element_id: palette.element_id, expected_revision: palette.revision, colors: Object.fromEntries(Object.entries(colors).filter(([from, to]) => from !== to)) }) as { id: string };
        if (isDirty() || getRevision() !== palette.revision || snapshot() !== observed) throw new Error("生成配色提案期间画布发生变化。提案已保留，请保存后在提案列表核对版本。");
        setOpen(false); onProposed(result.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>预览配色提案</Button></div>
  </DialogContent></Dialog>;
}
