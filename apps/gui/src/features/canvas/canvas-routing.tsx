import { useState } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { Button } from "@/components/ui/button";
import { gatewayRequest } from "@/gateway";

export function CanvasRouting({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: {
  projectId: string; canvasId: string; disabled: boolean;
  getApi: () => ExcalidrawImperativeAPI | null; getRevision: () => number | undefined;
  ensureSaved: () => Promise<boolean>; isDirty: () => boolean; onApplied: (id: string) => void;
}) {
  const { t } = useTranslation("canvas");
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  return <><Button size="sm" variant="outline" disabled={disabled || busy} title={t("routing.buttonTitle")} onClick={async () => {
    if (busy) return; setBusy(true); setError("");
    try {
      if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
      const api = getApi(), revision = getRevision();
      if (!api || revision === undefined || isDirty()) throw new Error(t("palette.canvasChangedSaveFirst"));
      const snapshot = () => serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
      const observed = snapshot();
      const operations = api.getSceneElements().filter(element => !element.isDeleted && element.type === "arrow" && element.customData?.crossPanel === true).map(element => ({ op: "reroute", id: element.id }));
      if (!operations.length) throw new Error(t("routing.noConnectors"));
      const result = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: revision, summary: t("routing.summary"), operations }) as { id: string };
      if (getRevision() !== revision || snapshot() !== observed || isDirty()) throw new Error(t("routing.canvasChangedDuringApply"));
      onApplied(result.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}>{busy ? t("routing.preparing") : t("routing.button")}</Button>{error && <span role="alert" className="text-xs text-destructive">{error}</span>}</>;
}
