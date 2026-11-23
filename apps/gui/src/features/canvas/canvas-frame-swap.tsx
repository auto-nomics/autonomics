import { useState, type ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Review = { revision: number; snapshot: string; frames: { id: string; title: string; locked: boolean }[] };
export function CanvasFrameSwap({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: ComponentProps<typeof CanvasInset>) {
  const { t } = useTranslation("canvas");
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [first, setFirst] = useState(""), [second, setSecond] = useState("");
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setError(""); setReview(null); setFirst(""); setSecond("");
      try {
        if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error(t("frameSwap.canvasChangedRetry"));
        const frames = api.getSceneElements().flatMap((element, index) => element.type === "frame" && !element.isDeleted && !element.frameId && element.angle === 0 ? [{ id: element.id, title: typeof element.customData?.title === "string" ? element.customData.title : element.name || t("frameSwap.defaultFrameTitle", { index: index + 1 }), locked: element.locked }] : []);
        if (frames.filter(frame => !frame.locked).length < 2) throw new Error(t("frameSwap.needTwoFrames"));
        setReview({ revision, snapshot: snapshot(), frames });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("frameSwap.button")}</Button>
    <DialogContent className="sm:max-w-lg"><DialogTitle>{t("frameSwap.title")}</DialogTitle>
      <p className="text-sm text-muted-foreground">{t("frameSwap.description")}</p>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && <form className="space-y-3 text-sm" onSubmit={async event => {
        event.preventDefault(); if (busy || !first || !second || first === second) return;
        setBusy(true); setError("");
        try {
          if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error(t("frameSwap.canvasChangedRecheck"));
          const result = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: review.revision, summary: t("frameSwap.summary"), operations: [{ op: "swap_frames", first_id: first, second_id: second }] }) as { id: string };

          setOpen(false); onApplied(result.id);
        } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
      }}>
        {[{ label: t("frameSwap.firstFrame"), value: first, other: second, set: setFirst }, { label: t("frameSwap.secondFrame"), value: second, other: first, set: setSecond }].map(field => <label key={field.label} className="block">{field.label}<select aria-label={field.label} className="mt-1 w-full rounded border bg-background p-2" value={field.value} disabled={busy} onChange={event => field.set(event.target.value)}><option value="">{t("frameSwap.selectFrame")}</option>{review.frames.map(frame => <option key={frame.id} value={frame.id} disabled={frame.locked || frame.id === field.other}>{frame.title}{frame.locked ? t("frameSwap.lockedSuffix") : ""}</option>)}</select></label>)}
        <Button type="submit" disabled={busy || !first || !second || first === second}>{busy ? t("frameSwap.preparing") : t("frameSwap.swap")}</Button>
      </form>}
    </DialogContent>
  </Dialog>;
}
