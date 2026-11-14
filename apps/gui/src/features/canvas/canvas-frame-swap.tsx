import { useState, type ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Review = { revision: number; snapshot: string; frames: { id: string; title: string; locked: boolean }[] };
export function CanvasFrameSwap({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: ComponentProps<typeof CanvasInset>) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [first, setFirst] = useState(""), [second, setSecond] = useState("");
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setError(""); setReview(null); setFirst(""); setSecond("");
      try {
        if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error("画布已变化，请保存后重试。");
        const frames = api.getSceneElements().flatMap((element, index) => element.type === "frame" && !element.isDeleted && !element.frameId && element.angle === 0 ? [{ id: element.id, title: typeof element.customData?.title === "string" ? element.customData.title : element.name || `分面 ${index + 1}`, locked: element.locked }] : []);
        if (frames.filter(frame => !frame.locked).length < 2) throw new Error("需要至少两个未锁定、未旋转的顶层分面。");
        setReview({ revision, snapshot: snapshot(), frames });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>交换分面位置</Button>
    <DialogContent className="sm:max-w-lg"><DialogTitle>交换分面位置</DialogTitle>
      <p className="text-sm text-muted-foreground">交换两个分面及其全部内容的位置，保留文字、尺寸、素材与科学关系，并重新整理受支持的跨面板连线。修改直接显示在画布中，可撤销。</p>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && <form className="space-y-3 text-sm" onSubmit={async event => {
        event.preventDefault(); if (busy || !first || !second || first === second) return;
        setBusy(true); setError("");
        try {
          if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error("画布已变化，请关闭面板后重新核对。");
          const result = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: review.revision, summary: "交换所选分面位置并整理连线", operations: [{ op: "swap_frames", first_id: first, second_id: second }] }) as { id: string };

          setOpen(false); onApplied(result.id);
        } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>
        {[{ label: "第一个分面", value: first, other: second, set: setFirst }, { label: "第二个分面", value: second, other: first, set: setSecond }].map(field => <label key={field.label} className="block">{field.label}<select aria-label={field.label} className="mt-1 w-full rounded border bg-background p-2" value={field.value} disabled={busy} onChange={event => field.set(event.target.value)}><option value="">请选择分面</option>{review.frames.map(frame => <option key={frame.id} value={frame.id} disabled={frame.locked || frame.id === field.other}>{frame.title}{frame.locked ? "（已锁定）" : ""}</option>)}</select></label>)}
        <Button type="submit" disabled={busy || !first || !second || first === second}>{busy ? "准备中…" : "交换分面"}</Button>
      </form>}
    </DialogContent>
  </Dialog>;
}
