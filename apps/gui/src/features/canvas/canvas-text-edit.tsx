import { useState } from "react";
import type { ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Review = { id: string; revision: number; snapshot: string; bound: boolean; fontSize: number };
export function CanvasTextEdit({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: ComponentProps<typeof CanvasInset>) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [text, setText] = useState(""), [width, setWidth] = useState(""), [fontSize, setFontSize] = useState("");
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setReview(null); setError("");
      try {
        if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请保存后重试。");
        const elements = api.getSceneElements().filter(element => !element.isDeleted), selected = api.getAppState().selectedElementIds;
        const chosen = elements.filter(element => selected[element.id] === true);
        const targets = elements.filter(element => element.type === "text" && (selected[element.id] === true || (element.containerId != null && selected[element.containerId] === true)));
        if (targets.length !== 1) throw new Error("请选中一段文字或一个带绑定文字的组件。");
        const target = targets[0];
        if (target.type !== "text" || chosen.some(element => element.id !== target.id && element.id !== target.containerId)) throw new Error("请只选中这一段文字及其容器。");
        const container = target.containerId ? elements.find(element => element.id === target.containerId) : undefined;
        if (target.containerId && (!container || container.boundElements?.filter(entry => entry.type === "text").length !== 1 || !container.boundElements.some(entry => entry.type === "text" && entry.id === target.id))) throw new Error("文字与容器的绑定不完整，请先修复绑定。");
        if (target.locked || container?.locked) throw new Error("请先解锁文字与容器。");
        if (target.fontFamily !== 9 || target.angle !== 0 || (container && container.angle !== 0)) throw new Error("此操作需要未旋转的科学字体文字及容器；其他文字可在画布中直接编辑。");
        setText(target.originalText ?? target.text); setWidth(String(target.width)); setFontSize(String(target.fontSize));
        setReview({ id: target.id, revision, snapshot: snapshot(), bound: target.containerId != null, fontSize: target.fontSize });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>编辑文字与换行</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>文字与换行</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && <div className="space-y-3 text-sm">
        <p className="break-all">文字：{review.id} · v{review.revision} · 科学字体 {review.fontSize}px</p>
        <label className="block">文字原文<textarea aria-label="文字原文" className="mt-1 min-h-32 w-full rounded border bg-background p-2" maxLength={2048} value={text} disabled={busy} onChange={event => setText(event.target.value)} /></label>
        <label className="block">字号<input aria-label="换行字号" className="ml-2 w-24 rounded border bg-background p-1" type="number" min="4" max="200" step="any" value={fontSize} disabled={busy} onChange={event => setFontSize(event.target.value)} /></label>
        {review.bound ? <p>按绑定容器的可用宽度换行。内容过高时，请先调整容器大小或字号。</p> : <label className="block">换行宽度（画布单位）<input aria-label="换行宽度" className="ml-2 w-24 rounded border bg-background p-1" type="number" min="1" max="20000" step="any" value={width} disabled={busy} onChange={event => setWidth(event.target.value)} /></label>}
        <p className="text-xs text-muted-foreground">保留字体、行高与其他样式。生成提案后检查换行、位置及与其他对象的重叠，再采用。</p>
        <Button disabled={busy || !text.trim() || !Number.isFinite(Number(fontSize)) || Number(fontSize) < 4 || Number(fontSize) > 200 || (!review.bound && (!Number.isFinite(Number(width)) || Number(width) < 1 || Number(width) > 20000))} onClick={async () => {
          setBusy(true); setError("");
          try {
            if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("画布发生变化，请重新打开文字编辑。");
            const candidate = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposals`, { expected_revision: review.revision, summary: "修改选中文字并按可用宽度换行", operations: [{ op: "set_wrapped_text", id: review.id, text, font_size: Number(fontSize), ...(!review.bound ? { max_width: Number(width) } : {}) }] }) as { id: string };
            if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("生成期间画布发生变化；提案已保留，请从列表重新核对。");
            setOpen(false); onProposed(candidate.id);
          } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
        }}>预览文字修改提案</Button>
      </div>}
    </DialogContent>
  </Dialog>;
}
