import { useState } from "react";
import { CaptureUpdateAction, convertToExcalidrawElements, newElementWith } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, BinaryFileData } from "@excalidraw/excalidraw/types";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { DEFAULT_DRAWING, renderCodeDrawing, type CodeDrawing } from "./canvas-code-runtime";
import { imagePlacement } from "./canvas-image-placement";

export function CanvasCodeDrawing({ getApi, disabled }: { getApi: () => ExcalidrawImperativeAPI | null; disabled?: boolean }) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [code, setCode] = useState(DEFAULT_DRAWING.code), [params, setParams] = useState(JSON.stringify(DEFAULT_DRAWING.params, null, 2));
  const [width, setWidth] = useState(480), [height, setHeight] = useState(320);
  const [target, setTarget] = useState<{ id: string; version: number } | null>(null);
  const [preview, setPreview] = useState<{ image: string; drawing: CodeDrawing; key: string } | null>(null);
  const key = JSON.stringify([code, params, width, height]);
  function begin() {
    const api = getApi(); if (!api) return;
    const selected = api.getSceneElements().filter(e => !e.isDeleted && api.getAppState().selectedElementIds[e.id]);
    const element = selected.length === 1 ? selected[0] : undefined;
    const drawing = element?.customData?.codeDrawing as CodeDrawing | undefined;
    setError(""); setPreview(null); setTarget(null);
    const initial = drawing?.version === 1 ? drawing : DEFAULT_DRAWING;
    setCode(initial.code); setParams(JSON.stringify(initial.params, null, 2)); setWidth(initial.width); setHeight(initial.height);
    if (drawing?.version === 1 && element) setTarget({ id: element.id, version: element.version });
    setOpen(true);
  }
  async function insert() {
    if (!preview || preview.key !== key) return;
    try {
      const api = getApi(); if (!api) throw new Error("画布未就绪。");
      const elements = api.getSceneElements();
      const existing = target ? elements.find(e => e.id === target.id && !e.isDeleted) : undefined;
      if (target && (!existing || existing.version !== target.version || existing.locked || existing.type !== "image")) throw new Error("原元素已修改、删除或锁定，请重新打开代码绘图。");
      if (existing && existing.type !== "image") throw new Error("请选择代码图片元素。");
      const fileId = crypto.randomUUID() as BinaryFileData["id"];
      const metadata = { ...(existing?.customData ?? {}), codeDrawing: preview.drawing };
      const placement = imagePlacement(api, width, height);
      const added = existing ? newElementWith(existing, { fileId, status: "saved", crop: null, customData: metadata }) : convertToExcalidrawElements([{ type: "image", x: placement.x, y: placement.y, width, height, fileId, status: "saved", customData: metadata }])[0];
      api.addFiles([{ id: fileId, dataURL: preview.image as BinaryFileData["dataURL"], mimeType: "image/png", created: Date.now() }]);
      api.updateScene({ elements: existing ? elements.map(e => e.id === existing.id ? added : e) : [...elements, added], appState: { selectedElementIds: { [added.id]: true } }, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
      if (!existing && placement.pan) api.scrollToContent([added], { animate: false });
      setOpen(false);
    } catch (cause) { setError(String(cause)); }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled} onClick={begin}>代码绘图</Button>
    <DialogContent className="max-h-[90vh] overflow-auto sm:max-w-4xl"><DialogTitle>{target ? "编辑代码元素" : "创建代码元素"}</DialogTitle>
      <p className="text-sm text-muted-foreground">定义 draw(ctx, params, width, height)，使用标准 Canvas 2D API。背景透明，代码和 JSON 参数随源文件保存；选中代码元素后可重新编辑。运行限时 4 秒，不支持联网及外部资源。</p>
      <div className="flex gap-4"><label>宽<input aria-label="绘图宽度" type="number" className="ml-2 w-24 rounded border p-1" value={width} onChange={e => setWidth(Number(e.target.value))}/></label><label>高<input aria-label="绘图高度" type="number" className="ml-2 w-24 rounded border p-1" value={height} onChange={e => setHeight(Number(e.target.value))}/></label></div>
      <label className="block">JavaScript<textarea aria-label="绘制函数" className="mt-1 block min-h-64 w-full rounded border bg-background p-2 font-mono text-sm" spellCheck={false} value={code} onChange={e => setCode(e.target.value)}/></label>
      <label className="block">JSON 参数<textarea aria-label="绘图参数" className="mt-1 block min-h-24 w-full rounded border bg-background p-2 font-mono text-sm" value={params} onChange={e => setParams(e.target.value)}/></label>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {preview?.key === key && <img alt="代码绘图预览" src={preview.image} className="max-h-64 max-w-full border bg-white object-contain"/>}
      <div className="flex gap-2"><Button disabled={busy} onClick={async () => {
        setBusy(true); setError(""); setPreview(null);
        try { const drawing: CodeDrawing = { version: 1, code, params: JSON.parse(params), width, height }; const image = await renderCodeDrawing(drawing); setPreview({ image, drawing, key }); }
        catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>{busy ? "正在绘制…" : "运行并预览"}</Button><Button disabled={busy || preview?.key !== key} onClick={() => void insert()}>{target ? "更新元素" : "插入画布"}</Button></div>
    </DialogContent>
  </Dialog>;
}
