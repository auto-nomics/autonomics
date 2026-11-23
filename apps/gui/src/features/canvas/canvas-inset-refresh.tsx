import { useState } from "react";
import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Status = { anchor_id: string; current_revision: number; source_revision: number; source_available: boolean; changed: boolean; file_changed?: boolean; geometry_changed?: boolean; metadata_changed?: boolean; detail_edited?: boolean };
export function CanvasInsetRefresh({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: ComponentProps<typeof CanvasInset>) {
  const { t } = useTranslation("canvas");
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [status, setStatus] = useState<Status | null>(null), [observed, setObserved] = useState(""), [discard, setDiscard] = useState(false);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
    setOpen(true); setBusy(true); setStatus(null); setError(""); setDiscard(false);
    try {
      if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error(t("common.canvasChangedRetry"));
      const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
      const anchors = elements.filter(element => !element.isDeleted && element.customData?.inset?.kind === "image_inset_snapshot" && element.customData?.inset?.anchor_id === element.id && (selected[element.id] || elements.some(child => selected[child.id] && child.id === element.customData?.visualAssetElementId && child.customData?.semanticAnchorId === element.id)));
      if (anchors.length !== 1) throw new Error(t("inset.selectOneInset"));
      const before = snapshot();
      const value = await gatewayRequest("GET", `${path}/inset-status?element_id=${encodeURIComponent(anchors[0].id)}&expected_revision=${revision}`) as Status;
      if (isDirty() || getRevision() !== revision || snapshot() !== before) throw new Error(t("insetRefresh.changedDuringCheck"));
      setObserved(before); setStatus(value);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>{t("insetRefresh.button")}</Button><DialogContent showCloseButton={false} className="w-[min(550px,95vw)]"><DialogTitle>{t("insetRefresh.title")}</DialogTitle>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {busy && !status && <p role="status">{t("insetRefresh.checking")}</p>}
    {status && <div className="space-y-3 text-sm"><p>{t("insetRefresh.revisions", { source: status.source_revision, current: status.current_revision })}</p>
      {!status.source_available ? <p role="status">{t("insetRefresh.sourceMissing")}</p> : <>
        <p role="status">{status.changed ? t("insetRefresh.sourceChanged") : t("insetRefresh.sourceUnchanged")}</p>
        {status.changed && <ul className="list-inside list-disc">{status.file_changed && <li>{t("insetRefresh.fileChanged")}</li>}{status.geometry_changed && <li>{t("insetRefresh.geometryChanged")}</li>}{status.metadata_changed && <li>{t("insetRefresh.metadataChanged")}</li>}</ul>}
        <p className="text-muted-foreground">{t("insetRefresh.syncNote")}</p><p className="text-muted-foreground">{t("insetRefresh.refreshNote")}</p>
        {status.detail_edited && <label className="flex items-start gap-2"><input type="checkbox" checked={discard} disabled={busy} onChange={event => setDiscard(event.target.checked)} />{t("insetRefresh.discardDetailEdits")}</label>}
      </>}
    </div>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>{t("insetRefresh.close")}</Button><Button size="sm" variant="outline" disabled={busy || !status?.source_available || status.file_changed} onClick={async () => {
      if (!status || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error(t("insetRefresh.canvasChangedRecheck"));
        const saved = await gatewayRequest("POST", `${path}/operations`, { expected_revision: status.current_revision, summary: t("insetRefresh.syncSummary"), operations: [{ op: "sync_inset_region", id: status.anchor_id }] }) as { id: string };

        setOpen(false); onApplied(saved.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("insetRefresh.syncOnly")}</Button><Button size="sm" disabled={busy || !status?.source_available || Boolean(status.detail_edited && !discard)} onClick={async () => {
      if (!status || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error(t("insetRefresh.canvasChangedRecheck"));
        const saved = await gatewayRequest("POST", `${path}/insets/refresh`, { element_id: status.anchor_id, expected_revision: status.current_revision, discard_detail_edits: discard }) as { id: string };

        setOpen(false); onApplied(saved.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("insetRefresh.apply")}</Button></div>
  </DialogContent></Dialog>;
}
