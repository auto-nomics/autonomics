import { useState } from "react";
import type { ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type TextTarget = { id: string; originalText: string };
type Review = { targets: TextTarget[]; arrow: boolean; id: string; revision: number; snapshot: string; bound: boolean; fontSize: number; originalText: string; width: number; canFit: boolean };
export function CanvasTextEdit({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: ComponentProps<typeof CanvasInset>) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [text, setText] = useState(""), [width, setWidth] = useState(""), [fontSize, setFontSize] = useState("");
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  const propose = async (summary: string, operations: Record<string, unknown>[]) => {
    if (!review) return;
    setBusy(true); setError("");
    try {
      if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("画布发生变化，请重新打开文字编辑。");
      const candidate = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/proposals`, { expected_revision: review.revision, summary, operations }) as { id: string };
      if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("生成期间画布发生变化；提案已保留，请从列表重新核对。");
      setOpen(false); onProposed(candidate.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  };
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
        if (targets.length === 0 || targets.length > 20) throw new Error("请选择 1–20 段文字或带绑定文字的组件。");
        if (chosen.some(element => !targets.some(target => target.type === "text" && (element.id === target.id || element.id === target.containerId)))) throw new Error("选区包含没有绑定文字的对象，请只选择文字或带文字的组件。");
        for (const target of targets) {
          if (target.type !== "text") throw new Error("选区包含不支持的文字对象。");
          const container = target.containerId ? elements.find(element => element.id === target.containerId) : undefined;
          if (target.containerId && (!container || container.boundElements?.filter(entry => entry.type === "text").length !== 1 || !container.boundElements.some(entry => entry.type === "text" && entry.id === target.id))) throw new Error(`文字 ${target.id} 与容器的绑定不完整，请先修复绑定。`);
          if (target.locked || container?.locked) throw new Error(`请先解锁文字 ${target.id} 与容器。`);
          if (target.fontFamily !== 9 || target.angle !== 0 || (container && container.angle !== 0)) throw new Error(`文字 ${target.id} 需要未旋转的科学字体及容器；其他文字可在画布中直接编辑。`);
          if (container && !["rectangle", "ellipse", "diamond", "arrow"].includes(container.type)) throw new Error(`文字 ${target.id} 的容器暂不支持重排。`);
        }
        const target = targets[0];
        if (target.type !== "text") throw new Error("请选择科学字体文字。");
        setText(target.originalText ?? target.text); setWidth(String(target.width)); setFontSize(String(target.fontSize));
        setReview({ targets: targets.map(item => { if (item.type !== "text") throw new Error("不支持的文字对象。"); return { id: item.id, originalText: item.originalText ?? item.text }; }), arrow: elements.some(item => item.id === target.containerId && item.type === "arrow"), id: target.id, revision, snapshot: snapshot(), bound: target.containerId != null, fontSize: target.fontSize, originalText: target.originalText ?? target.text, width: target.width,
          canFit: target.containerId == null && !target.boundElements?.length && !elements.some(element => element.type === "arrow" && (element.startBinding?.elementId === target.id || element.endBinding?.elementId === target.id)) });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>编辑文字与换行</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>文字与换行</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && review.targets.length > 1 && <section aria-label="批量文字重排" className="space-y-3 text-sm">
        <p>已选择 {review.targets.length} 段文字 · v{review.revision}</p>
        <p className="text-muted-foreground">保留每段原文、显式换行、字号与现有宽度；形状文字按容器内部空间排版。只重新计算自动换行，生成提案后检查重叠再采用。</p>
        <ul className="max-h-64 space-y-2 overflow-auto rounded border p-3">{review.targets.map((target, index) => <li key={target.id}><p className="text-xs text-muted-foreground">文字 {index + 1}</p><p className="whitespace-pre-wrap break-words">{target.originalText}</p></li>)}</ul>
        <Button disabled={busy} onClick={() => propose("仅重排选中文字，保留原文和字号", review.targets.map(target => ({ op: "set_wrapped_text", id: target.id, text: target.originalText })))}>预览批量文字重排</Button>
      </section>}
      {review && review.targets.length === 1 && <div className="space-y-3 text-sm">
        <p className="break-all">文字：{review.id} · v{review.revision} · 科学字体 {review.fontSize}px</p>
        <label className="block">文字原文<textarea aria-label="文字原文" className="mt-1 min-h-32 w-full rounded border bg-background p-2" maxLength={2048} value={text} disabled={busy} onChange={event => setText(event.target.value)} /></label>
        <label className="block">字号<input aria-label="换行字号" className="ml-2 w-24 rounded border bg-background p-1" type="number" min="4" max="200" step="any" value={fontSize} disabled={busy} onChange={event => setFontSize(event.target.value)} /></label>
        {review.bound && !review.arrow ? <p>按绑定容器的可用宽度换行。内容过高时，请先调整容器大小或字号。</p> : <label className="block">换行宽度（画布单位）<input aria-label="换行宽度" className="ml-2 w-24 rounded border bg-background p-1" type="number" min="1" max="20000" step="any" value={width} disabled={busy} onChange={event => setWidth(event.target.value)} /></label>}
        <p className="text-xs text-muted-foreground">保留字体、行高与其他样式。生成提案后检查换行、位置及与其他对象的重叠，再采用。</p>
        {review.arrow && <p className="text-xs text-muted-foreground">只调整连线标签的换行宽度，保留箭头几何与绑定。请在预览中核对是否遮挡其他对象。</p>}
        <Button disabled={busy || !text.trim() || !Number.isFinite(Number(fontSize)) || Number(fontSize) < 4 || Number(fontSize) > 200 || ((!review.bound || review.arrow) && (!Number.isFinite(Number(width)) || Number(width) < 1 || Number(width) > 20000))} onClick={() => propose("修改选中文字并按可用宽度换行", [{ op: "set_wrapped_text", id: review.id, text, font_size: Number(fontSize), ...(!review.bound || review.arrow ? { max_width: Number(width) } : {}) }])}>预览文字修改提案</Button>
        {!review.bound && <div className="space-y-2 border-t pt-3">
          <p className="text-xs text-muted-foreground">收紧现有框宽会保留当前文字、换行和可见对齐。修改上方输入后，请先预览文字修改。</p>
          {!review.canFit && <p className="text-xs text-muted-foreground">附有连线的文字框不能直接收紧。</p>}
          <Button variant="outline" disabled={busy || !review.canFit || text !== review.originalText || Number(width) !== review.width || Number(fontSize) !== review.fontSize} onClick={() => propose("收紧选中文字框宽度", [{ op: "fit_text_width", id: review.id }])}>预览收紧文字框</Button>
        </div>}
      </div>}
    </DialogContent>
  </Dialog>;
}
