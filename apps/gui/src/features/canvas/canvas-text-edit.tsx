import { useState } from "react";
import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type TextTarget = { id: string; originalText: string };
type Review = { targets: TextTarget[]; arrow: boolean; id: string; revision: number; snapshot: string; bound: boolean; fontSize: number; originalText: string; width: number; canFit: boolean };
export function CanvasTextEdit({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: ComponentProps<typeof CanvasInset>) {
  const { t } = useTranslation("canvas");
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [text, setText] = useState(""), [width, setWidth] = useState(""), [fontSize, setFontSize] = useState("");
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  const applyEdit = async (summary: string, operations: Record<string, unknown>[]) => {
    if (!review) return;
    setBusy(true); setError("");
    try {
      if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error(t("textEdit.canvasChanged"));
      const saved = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: review.revision, summary, operations }) as { id: string };

      setOpen(false); onApplied(saved.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setReview(null); setError("");
      try {
        if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error(t("common.canvasChangedRetry"));
        const elements = api.getSceneElements().filter(element => !element.isDeleted), selected = api.getAppState().selectedElementIds;
        const chosen = elements.filter(element => selected[element.id] === true);
        const targets = elements.filter(element => element.type === "text" && (selected[element.id] === true || (element.containerId != null && selected[element.containerId] === true)));
        if (targets.length === 0 || targets.length > 20) throw new Error(t("textEdit.selectRange"));
        if (chosen.some(element => !targets.some(target => target.type === "text" && (element.id === target.id || element.id === target.containerId)))) throw new Error(t("textEdit.selectionIncludesOthers"));
        for (const target of targets) {
          if (target.type !== "text") throw new Error(t("textEdit.unsupportedTextObject"));
          const container = target.containerId ? elements.find(element => element.id === target.containerId) : undefined;
          if (target.containerId && (!container || container.boundElements?.filter(entry => entry.type === "text").length !== 1 || !container.boundElements.some(entry => entry.type === "text" && entry.id === target.id))) throw new Error(t("textEdit.incompleteBinding", { id: target.id }));
          if (target.locked || container?.locked) throw new Error(t("textEdit.locked", { id: target.id }));
          if (target.fontFamily !== 9 || target.angle !== 0 || (container && container.angle !== 0)) throw new Error(t("textEdit.needsScientificFont", { id: target.id }));
          if (container && !["rectangle", "ellipse", "diamond", "arrow"].includes(container.type)) throw new Error(t("textEdit.containerUnsupported", { id: target.id }));
        }
        const target = targets[0];
        if (target.type !== "text") throw new Error(t("textEdit.selectScientificText"));
        setText(target.originalText ?? target.text); setWidth(String(target.width)); setFontSize(String(target.fontSize));
        setReview({ targets: targets.map(item => { if (item.type !== "text") throw new Error(t("textEdit.unsupportedTextObject")); return { id: item.id, originalText: item.originalText ?? item.text }; }), arrow: elements.some(item => item.id === target.containerId && item.type === "arrow"), id: target.id, revision, snapshot: snapshot(), bound: target.containerId != null, fontSize: target.fontSize, originalText: target.originalText ?? target.text, width: target.width,
          canFit: target.containerId == null && !target.boundElements?.length && !elements.some(element => element.type === "arrow" && (element.startBinding?.elementId === target.id || element.endBinding?.elementId === target.id)) });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("textEdit.button")}</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>{t("textEdit.title")}</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && review.targets.length > 1 && <section aria-label={t("textEdit.batchLabel")} className="space-y-3 text-sm">
        <p>{t("textEdit.batchSummary", { count: review.targets.length, revision: review.revision })}</p>
        <p className="text-muted-foreground">{t("textEdit.batchNote")}</p>
        <ul className="max-h-64 space-y-2 overflow-auto rounded border p-3">{review.targets.map((target, index) => <li key={target.id}><p className="text-xs text-muted-foreground">{t("textEdit.textN", { index: index + 1 })}</p><p className="whitespace-pre-wrap break-words">{target.originalText}</p></li>)}</ul>
        <Button disabled={busy} onClick={() => applyEdit(t("textEdit.batchSummaryOperation"), review.targets.map(target => ({ op: "set_wrapped_text", id: target.id, text: target.originalText })))}>{t("textEdit.applyBatch")}</Button>
      </section>}
      {review && review.targets.length === 1 && <div className="space-y-3 text-sm">
        <p className="break-all">{t("textEdit.singleSummary", { id: review.id, revision: review.revision, fontSize: review.fontSize })}</p>
        <label className="block">{t("textEdit.originalText")}<textarea aria-label={t("textEdit.originalText")} className="mt-1 min-h-32 w-full rounded border bg-background p-2" maxLength={2048} value={text} disabled={busy} onChange={event => setText(event.target.value)} /></label>
        <label className="block">{t("textEdit.fontSize")}<input aria-label={t("textEdit.fontSizeLabel")} className="ml-2 w-24 rounded border bg-background p-1" type="number" min="4" max="200" step="any" value={fontSize} disabled={busy} onChange={event => setFontSize(event.target.value)} /></label>
        {review.bound && !review.arrow ? <p>{t("textEdit.boundWrapNote")}</p> : <label className="block">{t("textEdit.wrapWidth")}<input aria-label={t("textEdit.wrapWidth")} className="ml-2 w-24 rounded border bg-background p-1" type="number" min="1" max="20000" step="any" value={width} disabled={busy} onChange={event => setWidth(event.target.value)} /></label>}
        <p className="text-xs text-muted-foreground">{t("textEdit.styleNote")}</p>
        {review.arrow && <p className="text-xs text-muted-foreground">{t("textEdit.arrowNote")}</p>}
        <Button disabled={busy || !text.trim() || !Number.isFinite(Number(fontSize)) || Number(fontSize) < 4 || Number(fontSize) > 200 || ((!review.bound || review.arrow) && (!Number.isFinite(Number(width)) || Number(width) < 1 || Number(width) > 20000))} onClick={() => applyEdit(t("textEdit.summaryOperation"), [{ op: "set_wrapped_text", id: review.id, text, font_size: Number(fontSize), ...(!review.bound || review.arrow ? { max_width: Number(width) } : {}) }])}>{t("textEdit.apply")}</Button>
        {!review.bound && <div className="space-y-2 border-t pt-3">
          <p className="text-xs text-muted-foreground">{t("textEdit.fitNote")}</p>
          {!review.canFit && <p className="text-xs text-muted-foreground">{t("textEdit.cannotFit")}</p>}
          <Button variant="outline" disabled={busy || !review.canFit || text !== review.originalText || Number(width) !== review.width || Number(fontSize) !== review.fontSize} onClick={() => applyEdit(t("textEdit.fitSummaryOperation"), [{ op: "fit_text_width", id: review.id }])}>{t("textEdit.applyFit")}</Button>
        </div>}
      </div>}
    </DialogContent>
  </Dialog>;
}
