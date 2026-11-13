import { useState } from "react";
import type { ComponentProps } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Status = { anchor_id: string; current_revision: number; source_revision: number; source_available: boolean; changed: boolean; file_changed?: boolean; geometry_changed?: boolean; metadata_changed?: boolean; detail_edited?: boolean };
export function CanvasInsetRefresh({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onProposed }: ComponentProps<typeof CanvasInset>) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [status, setStatus] = useState<Status | null>(null), [observed, setObserved] = useState(""), [discard, setDiscard] = useState(false);
  const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
    setOpen(true); setBusy(true); setStatus(null); setError(""); setDiscard(false);
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请保存后重试。");
      const elements = api.getSceneElements(), selected = api.getAppState().selectedElementIds;
      const anchors = elements.filter(element => !element.isDeleted && element.customData?.inset?.kind === "image_inset_snapshot" && element.customData?.inset?.anchor_id === element.id && (selected[element.id] || elements.some(child => selected[child.id] && child.id === element.customData?.visualAssetElementId && child.customData?.semanticAnchorId === element.id)));
      if (anchors.length !== 1) throw new Error("请只选中一个放大图的内容组或边框。");
      const before = snapshot();
      const value = await gatewayRequest("GET", `${path}/inset-status?element_id=${encodeURIComponent(anchors[0].id)}&expected_revision=${revision}`) as Status;
      if (isDirty() || getRevision() !== revision || snapshot() !== before) throw new Error("检查期间画布发生变化，请重新打开。");
      setObserved(before); setStatus(value);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>检查并刷新放大图</Button><DialogContent showCloseButton={false} className="w-[min(550px,95vw)]"><DialogTitle>检查放大图来源</DialogTitle>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {busy && !status && <p role="status">检查源图变化…</p>}
    {status && <div className="space-y-3 text-sm"><p>快照来源版本：v{status.source_revision}；当前画布：v{status.current_revision}</p>
      {!status.source_available ? <p role="status">源图已删除或缺失。快照仍保留原内容，请恢复源图后再刷新。</p> : <>
        <p role="status">{status.changed ? "源图已发生变化" : "未检测到源图变化"}</p>
        {status.changed && <ul className="list-inside list-disc">{status.file_changed && <li>图片文件已变化</li>}{status.geometry_changed && <li>源图位置、变换或裁剪已变化</li>}{status.metadata_changed && <li>素材来源信息已变化</li>}</ul>}
        <p className="text-muted-foreground">“仅同步区域标注”保留快照和标题，按原始裁剪像素重新定位区域框与引导线；原文件须保持一致，原区域须在当前裁剪内。不会自动跟随源图。</p><p className="text-muted-foreground">保留放大图的边框、标题和样式，将原比例区域应用到当前源图，并重新定位区域框与虚线。图片尺寸会适配现有边框。更换源图后，请在提案中核对放大的科学部位。</p>
        {status.detail_edited && <label className="flex items-start gap-2"><input type="checkbox" checked={discard} disabled={busy} onChange={event => setDiscard(event.target.checked)} />用当前源图替换放大图单独修改过的文件或裁剪</label>}
      </>}
    </div>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>关闭来源检查</Button><Button size="sm" variant="outline" disabled={busy || !status?.source_available || status.file_changed} onClick={async () => {
      if (!status || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error("画布发生变化，请重新检查源图。");
        const proposal = await gatewayRequest("POST", `${path}/proposals`, { expected_revision: status.current_revision, summary: "同步放大图区域标注", operations: [{ op: "sync_inset_region", id: status.anchor_id }] }) as { id: string };
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error("生成期间画布发生变化；提案已保留，请核对版本。");
        setOpen(false); onProposed(proposal.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>仅同步区域标注</Button><Button size="sm" disabled={busy || !status?.source_available || Boolean(status.detail_edited && !discard)} onClick={async () => {
      if (!status || busy) return; setBusy(true); setError("");
      try {
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error("画布发生变化，请重新检查源图。");
        const proposal = await gatewayRequest("POST", `${path}/insets/refresh`, { element_id: status.anchor_id, expected_revision: status.current_revision, discard_detail_edits: discard }) as { id: string };
        if (isDirty() || getRevision() !== status.current_revision || snapshot() !== observed) throw new Error("生成期间画布发生变化；提案已保留，请核对版本。");
        setOpen(false); onProposed(proposal.id);
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>预览刷新提案</Button></div>
  </DialogContent></Dialog>;
}
