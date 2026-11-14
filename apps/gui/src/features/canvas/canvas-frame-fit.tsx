import { useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { Button } from "@/components/ui/button";
import { gatewayRequest } from "@/gateway";

export function CanvasFrameFit({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: {
  projectId: string; canvasId: string; disabled: boolean;
  getApi: () => ExcalidrawImperativeAPI | null; getRevision: () => number | undefined;
  ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onApplied: (id: string) => void;
}) {
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  return <><Button size="sm" variant="outline" disabled={disabled || busy} title="按现有内容收紧所有顶层分面的右侧和底部留白，尽量保留24单位边距；不移动内容，直接更新画布，可撤销" onClick={async () => {
    if (busy) return; setBusy(true); setError("");
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请先保存后重试。");
      const snapshot = () => serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
      const observed = snapshot();
      const operations = api.getSceneElements().filter(element => !element.isDeleted && element.type === "frame" && !element.frameId).map(element => ({ op: "fit_frame", id: element.id, padding: 24 }));
      if (!operations.length) throw new Error("当前没有可收紧的顶层分面。");
      const result = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: revision, summary: "按内容收紧分面留白", operations }) as { id: string };
      if (getRevision() !== revision || snapshot() !== observed || isDirty()) throw new Error("收紧期间画布发生变化。修改已保留，请保存后通过“查看 Agent 修改”核对版本。");
      onApplied(result.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>{busy ? "准备中…" : "收紧所有分面"}</Button>{error && <span role="alert" className="text-xs text-destructive">{error}</span>}</>;
}
