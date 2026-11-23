import { useCanvasIllustratorPath } from "./canvas-illustrator-context";
import { appendCanvasRequestDraft } from "./canvas-request-draft";
import { svgPathReadInputs } from "./canvas-svg-path-context";
import { useSvgPathInspection } from "./canvas-svg-path-channel";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";

type PathInspection = {
  revision: number; sha256: string; file_id: string; element_id: string;
  render_contribution?: unknown;
  parts: { part_id: string; index: number; bounds: number[]; fill: string; stroke: string; stroke_width: number; opacity: number; changed_preview_pixels_when_hidden?: number }[];
};
export function CanvasPathAgent({ projectId, canvasId, elementId, revision, sha256, partId, partIndex, busy, unchanged, onBusy, onError, onPrepared, capturePreview }: {
  capturePreview: () => Promise<boolean>; projectId: string; canvasId: string; elementId: string; revision: number; sha256: string; partId: string; partIndex: number;
  busy: boolean; unchanged: () => boolean; onBusy: (busy: boolean) => void; onError: (error: string) => void; onPrepared: () => void;
}) {
  const { t } = useTranslation("canvas");
  useSvgPathInspection({project_id:projectId,figure_id:canvasId,element_id:elementId,expected_revision:revision,sha256,part_id:partId,part_index:partIndex});
  const runtime = useWorkbenchRuntime();
  const runtimeRef = useRef(runtime); runtimeRef.current = runtime;
  const target = useCanvasIllustratorPath();
  const [request, setRequest] = useState("");
  const targetAvailable = runtime.agents.some(agent => agent.path === target);
  return <section data-svg-part-id={partId} aria-label={t("pathAgent.sectionLabel")} className="mt-3 space-y-2 border-t pt-3 text-sm">
    <p>{t("pathAgent.description")}</p>
    <textarea aria-label={t("pathAgent.requestLabel")} disabled={busy} value={request} className="min-h-20 w-full rounded border bg-background p-2" placeholder={t("pathAgent.requestPlaceholder")} onChange={event => setRequest(event.target.value)} />
    <Button size="sm" disabled={busy || !targetAvailable || !request.trim()} onClick={async () => {
      if (busy || !targetAvailable || !request.trim()) return;
      onBusy(true); onError("");
      try {
        if (!unchanged()) throw new Error(t("pathAgent.canvasChanged"));
        if (!runtime.agents.some(agent => agent.path === target)) throw new Error(t("pathAgent.agentUnavailable"));
        const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
        const latest = await gatewayRequest("GET", `${path}/vector-parts?element_id=${encodeURIComponent(elementId)}&expected_revision=${revision}`) as PathInspection;
        const part = latest.parts.find(part => part.part_id === partId);
        if (!unchanged() || latest.revision !== revision || latest.sha256 !== sha256 || latest.element_id !== elementId || !part) throw new Error(t("pathAgent.versionChanged"));
        const previewAvailable = await capturePreview();
        if (!unchanged()) throw new Error(t("pathAgent.previewChanged"));
        const scope = { project_id: projectId, figure_id: canvasId, expected_revision: revision, element_id: elementId };
        const context = {
          prepared_request: request.trim(), preview_available: previewAvailable, scope: "saved_svg_path", ...scope, file_id: latest.file_id, sha256,
          selected_path: { part_id: part.part_id, index: part.index, bounds: part.bounds, fill: part.fill, stroke: part.stroke, stroke_width: part.stroke_width, opacity: part.opacity, changed_preview_pixels_when_hidden: part.changed_preview_pixels_when_hidden },
          render_contribution: latest.render_contribution,
          follow_up_inputs: svgPathReadInputs({project_id:projectId,figure_id:canvasId,element_id:elementId,expected_revision:revision,sha256,part_id:partId,part_index:partIndex},previewAvailable),
        };
        const prompt = ["按用户请求直接修改当前画布的指定 SVG 路径。先 read 和 parts 核对版本、sha256 与路径编号，需要控制点时读取 geometry。用 bioicons edit_parts 直接保存修改，继续使用返回的新 revision，修改后读取 parts 和画布 PNG。保留其他对象、路径、来源与许可，不将局部投影覆盖整图。路径编号不代表生物学身份。", JSON.stringify(context, null, 2)].join("\n\n");
        if (!runtimeRef.current.agents.some(agent => agent.path === target)) throw new Error(t("pathAgent.agentUnavailableKept"));
        runtime.setDraftFor(target, previous => appendCanvasRequestDraft(previous, request, { title: t("pathAgent.draftTitle", { index: part.index }), source: `visualization:${projectId}/${canvasId}/images/${elementId}/parts/${partId}`, revision: `${revision}:${sha256}`, prompt }));
        runtime.selectAgent(target); openAgentEditor(target); onPrepared();
      } catch (cause) { onError(String(cause)); } finally { onBusy(false); }
    }}>{t("pathAgent.prepare")}</Button>
  </section>;
}
