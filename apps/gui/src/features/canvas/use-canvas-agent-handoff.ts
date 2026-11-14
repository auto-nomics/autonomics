import { appendCanvasRequestDraft } from "./canvas-request-draft";
import { captionGuidance, layoutGuidance, type LayoutPreference, type CaptionStyle } from "./canvas-composition";
import { useRef, useState } from "react";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import type { MarkupPage, MarkupRecord, MarkupSummary } from "./canvas-markup-render";
import { canvasMaterializationProgress, canvasReadInput, canvasSelectionContext } from "./canvas-selection-context";
import { createSourceManifest } from "./canvas-source-manifest";
import type { Annotation } from "./canvas-annotations";
import { GatewayHttpError, gatewayRequest } from "@/gateway";
export type ReviewedMarkupScene = { revision: number; serialized: string };
export type CanvasAgentHandoffProps = {
  projectId: string; canvasId: string; getRevision: () => number | undefined;
  getApi: () => ExcalidrawImperativeAPI | null; ensureSaved: () => Promise<boolean>; capturePreview: () => Promise<boolean>; isDirty: () => boolean;
};
export function useCanvasAgentHandoff({ projectId, canvasId, getRevision, getApi, ensureSaved, capturePreview, isDirty, target, audience = "", captionStyle = "concise", layoutPreference = "auto" }: CanvasAgentHandoffProps & { target: string; audience?: string; captionStyle?: CaptionStyle; layoutPreference?: LayoutPreference }) {
  const runtime = useWorkbenchRuntime();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const runtimeRef = useRef(runtime); runtimeRef.current = runtime;
  const targetAvailable = runtime.agents.some(agent => agent.path === target);
  const prepare = async (requestText: string, annotation?: Annotation, markup?: MarkupSummary, reviewedMarkupScene?: ReviewedMarkupScene): Promise<boolean> => {
      if (busy || !targetAvailable || !requestText.trim()) return false;
      setBusy(true); setError("");
      try {
        if (!await ensureSaved()) throw new Error("画布未能保存，请先解决保存错误。");
        const api = getApi(); const revision = getRevision();
        if (!api || revision === undefined) throw new Error("画布尚未就绪");
        const previewAvailable = api.getSceneElements().length > 0 ? await capturePreview() : false;
        if (api.getSceneElements().length > 0 && !previewAvailable) throw new Error("画布预览未能生成，请重试。");
        const observed = serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local");
        const selected = markup ? {} : annotation ? Object.fromEntries(annotation.element_ids.map(id => [id, true])) : api.getAppState().selectedElementIds;
        if (annotation && (annotation.at_revision !== revision || annotation.element_ids.some(id => !api.getSceneElements().some(element => element.id === id && !element.isDeleted)))) throw new Error("批注对应的画布版本或对象已变化，请重新选择并建立批注。");
        const selection = canvasSelectionContext(api.getSceneElements(), selected);
        const objects = selection.objects;
        const allAnnotations = markup ? [] : await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/annotations`) as Annotation[];
        if (annotation && !allAnnotations.some(note => note.id === annotation.id && note.status === "open" && note.at_revision === revision && note.note === annotation.note && JSON.stringify(note.element_ids) === JSON.stringify(annotation.element_ids))) throw new Error("批注已变化或已处理，请刷新后核对。");
        const annotations = markup ? [] : annotation ? [annotation] : allAnnotations.filter(note => note.status === "open").slice(0, 30);
        let reviewPage: MarkupPage;
        if (markup) {
          const latest = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/markups/${encodeURIComponent(markup.id)}`) as MarkupRecord;
          if (latest.id !== markup.id || latest.status !== "open" || latest.note !== markup.note || latest.stroke_count !== markup.stroke_count || latest.at_revision !== markup.at_revision) throw new Error("草图标记已变化或已处理，请刷新后核对。");
          if (latest.at_revision !== revision && (reviewedMarkupScene?.revision !== revision || reviewedMarkupScene.serialized !== observed)) throw new Error("请先对照历史标记与当前图稿并确认；图稿变化后须重新核对。");
          reviewPage = { items: [{ id: latest.id, at_revision: latest.at_revision, note: latest.note, stroke_count: latest.stroke_count, status: latest.status }], next_before: null };
        } else reviewPage = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/markups?page=true&status=open`) as MarkupPage;
        const markups = reviewPage.items;
        const manifest = await createSourceManifest({ scene: observed, title: "", project_id: projectId, figure_id: canvasId, based_on_revision: revision, unsaved_changes: false });
        const assetSources = { total: manifest.assets.length, truncated: manifest.assets.length > 30, assets: [...manifest.assets].sort((a,b) => Number(Boolean(selected[b.element_id ?? ""] || selected[b.semantic_anchor_id ?? ""] || selected[b.semantic_badge_anchor_id ?? ""])) - Number(Boolean(selected[a.element_id ?? ""] || selected[a.semantic_anchor_id ?? ""] || selected[a.semantic_badge_anchor_id ?? ""]))).slice(0, 30).map(asset => ({ element_id: asset.element_id, semantic_anchor_id: asset.semantic_anchor_id, semantic_badge_anchor_id:asset.semantic_badge_anchor_id,badge_meaning:asset.badge_meaning,badge_ownership_verified:asset.badge_ownership_verified,badge_scientific_meaning_verified:asset.badge_scientific_meaning_verified, mime_type: asset.mime_type, provider: asset.provider, name: asset.name, authors: asset.authors, licenses: asset.licenses, license_urls: asset.license_urls, source_urls: asset.source_urls, actual_sha256: asset.actual_sha256, hash_matches_recorded: asset.hash_matches_recorded, modifications: asset.modifications.slice(-3), missing_fields: asset.missing_fields })) };
        const head = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`) as { metadata: { revision: number; title?: string } };
        if (head.metadata.revision !== revision) throw new Error("服务端画布已更新，请重新加载后核对批注与修改需求。");
        if (isDirty() || getRevision() !== revision || serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") !== observed) throw new Error("交接期间画布发生变化，请保存后重试。");
        const prompt = [
          ...(annotation ? ["仅处理 focused_annotation_id 指定的对象批注。先读取当前画布，再通过 apply 或专用文字工具直接修改；对象批注与草图标记是独立反馈，保留未处理的意见。"] : []),
          ...(markup ? ["本轮仅处理 focused_markup_id 的草图标记。标记图片可能属于历史版本，请用 read_markup 查看时间并与当前图稿核对。只修改当前仍存在的对应对象，保留其他意见；不要自动完成反馈。"] : []),
          captionGuidance(captionStyle),
          layoutGuidance(layoutPreference),
          "如果物种、范围或终点尚不明确，先向用户提问并核对科学来源，不要猜测。",
          "直接编辑当前科研画布：先 read 核对版本与选区，使用 apply 批量修改，后续编辑使用返回的 revision。保留无关对象、完整科学标签和来源。追加图形用 insert_sketch，只有明确要求替换整图才用 sketch 加 replace_existing:true。素材先 search/preview 再 insert/replace，SVG 局部编辑用 parts/geometry/edit_parts。修改后用 read_preview 检查整图，不需人工采用。缺少科学信息时只澄清必要内容。",
          JSON.stringify({ materialization_checkpoint: canvasMaterializationProgress(api.getSceneElements()), follow_up_inputs: { read: canvasReadInput(projectId, canvasId, revision, selection), ...(markup ? { read_markup: { action: "read_markup", project_id: projectId, figure_id: canvasId, markup_id: markup.id, expected_revision: revision } } : {}) }, project_id: projectId, figure_id: canvasId, observed_revision: revision, audience: audience.trim(), composition_preferences: { caption_style: captionStyle, layout_preference: layoutPreference, preserve_complete_sources: true, preserve_confirmed_scientific_labels: true }, prepared_request: requestText.trim(), focused_annotation_id: annotation?.id, focused_markup_id: markup?.id, ...(markup ? { markup_revision_comparison: { marked_revision: markup.at_revision, current_revision: revision, historical: markup.at_revision !== revision, user_confirmed_comparison: markup.at_revision !== revision && reviewedMarkupScene?.revision === revision } } : {}), review_channels: { annotations: { kind: "object_annotation" }, markups: { kind: "sketch_markup" } }, selected_objects: objects, selected_objects_total: selection.total, selected_objects_truncated: selection.truncated, annotations, markups, markups_pagination: { status: "open", next_before: reviewPage.next_before }, asset_sources: assetSources, preview_available: previewAvailable }, null, 2),
        ].join("\n\n");
        if (!runtimeRef.current.agents.some(agent => agent.path === target)) throw new Error("选定 Agent 已不可用，请重新选择后重试。");
        runtime.setDraftFor(target, previous => appendCanvasRequestDraft(previous, requestText, { title: `绘图请求上下文 · ${head.metadata.title || "当前画布"}`, source: `visualization:${projectId}/${canvasId}`, revision: String(revision), prompt }));
        runtime.selectAgent(target); openAgentEditor(target);
        return true;
      } catch (cause) { setError(cause instanceof GatewayHttpError && cause.status === 409 ? "服务端画布已更新，请重新加载后核对批注与修改需求。" : String(cause)); return false; } finally { setBusy(false); }
  };
  return { prepare, busy, error, targetAvailable, clearError: () => setError("") };
}
