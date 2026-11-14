import { useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { Button } from "@/components/ui/button";
import { gatewayRequest } from "@/gateway";

export function CanvasRouting({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: {
  projectId: string; canvasId: string; disabled: boolean;
  getApi: () => ExcalidrawImperativeAPI | null; getRevision: () => number | undefined;
  ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onApplied: (id: string) => void;
}) {
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  return <><Button size="sm" variant="outline" disabled={disabled || busy} title="根据当前位置为生成的跨面板连线重新避障，直接更新画布，可撤销" onClick={async () => {
    if (busy) return; setBusy(true); setError("");
    try {
      if (!await ensureSaved()) throw new Error("请先解决画布保存错误。");
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error("画布发生变化，请先保存后重试。");
      const snapshot = () => serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
      const observed = snapshot();
      const operations = api.getSceneElements().filter(element => !element.isDeleted && element.type === "arrow" && element.customData?.crossPanel === true).map(element => ({ op: "reroute", id: element.id }));
      if (!operations.length) throw new Error("当前没有可整理的跨面板连线。");
      const result = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: revision, summary: "根据当前位置整理跨面板连线", operations }) as { id: string };
      if (getRevision() !== revision || snapshot() !== observed || isDirty()) throw new Error("整理期间画布发生变化。修改已保留，请保存后通过“查看 Agent 修改”核对版本。");
      onApplied(result.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>{busy ? "整理中…" : "整理跨面板连线"}</Button>{error && <span role="alert" className="text-xs text-destructive">{error}</span>}</>;
}
