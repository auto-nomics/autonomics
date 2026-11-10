import { useEffect, useRef, useState } from "react";
import { Excalidraw, MainMenu, exportToBlob, exportToSvg, serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import "@excalidraw/excalidraw/index.css";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { patchEditorState } from "@/app/workbench/workbench-store";
import type { EditorContext } from "@/app/workbench/editor-registry";
import { canvasSelectionContext } from "./canvas-selection-context";
import { createSourceManifest, embedSvgSources, embedPngSources } from "./canvas-source-manifest";
import { CanvasInset } from "./canvas-inset";
import { CanvasPalette } from "./canvas-palette";
import { CanvasRelation } from "./canvas-relation";
import { CanvasRouting } from "./canvas-routing";
import { CanvasPlanning } from "./canvas-planning";
import { CanvasMarkups } from "./canvas-markups";
import { CanvasHistory } from "./canvas-history";
import { CanvasCircleSelect } from "./canvas-circle-select";
import { CanvasProposals } from "./canvas-proposals";
import { CanvasAgentPanel } from "./canvas-agent-panel";
import { BioIconsPanel } from "./bioicons-panel";
import { BioArtPanel } from "./bioart-panel";
import { readCanvas, saveCanvas } from "./canvas-store";

function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a"); link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
export function CanvasEditor(context: EditorContext) {
  const [generation, setGeneration] = useState(0);
  return <CanvasEditorInstance key={`${context.editor.id}:${generation}`} {...context} onReload={() => setGeneration(value => value + 1)} />;
}
function CanvasEditorInstance({ editor, onReload }: EditorContext & { onReload: () => void }) {
  const id = String(editor.params.canvasId);
  const projectId = typeof editor.params.projectId === "string" ? editor.params.projectId : undefined;
  const revision = useRef<number | undefined>(undefined);
  const api = useRef<ExcalidrawImperativeAPI | null>(null);
  const baseline = useRef<string | null>(null);
  const fitted = useRef(false);
  const titleRef = useRef("");
  const saving = useRef(false);
  const latestTitle = useRef("未命名画布");
  const [title, setTitle] = useState("未命名画布");
  const [error, setError] = useState("");
  const [circleSelect, setCircleSelect] = useState(false);
  const [proposalRequest, setProposalRequest] = useState<{ id: string }>();
  const [showAgent, setShowAgent] = useState(false);
  const [showBioArt, setShowBioArt] = useState(false);
  const [showBioIcons, setShowBioIcons] = useState(false);
  const [imagesReady, setImagesReady] = useState(true);
  const [busy, setBusy] = useState(false);
  const [initial, setInitial] = useState<ExcalidrawInitialDataState | null>(null);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    let cancelled = false;
    patchEditorState(editor.id, { selectedObjectIds: "[]", selectedObjects: "[]", selectedObjectsTotal: 0, selectedObjectsTruncated: false, selectionReady: false });
    void readCanvas(id, projectId).then(record => {
      if (cancelled) return;
      revision.current = record.revision;
      titleRef.current = record.title; latestTitle.current = record.title; setTitle(record.title);
      setInitial(record.scene ? { ...JSON.parse(record.scene), scrollToContent: true } : { appState: { currentItemRoughness: 0, currentItemFontFamily: 9, currentItemRoundness: "sharp", viewBackgroundColor: "#ffffff" } });
      setLoaded(true);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; setDocumentDirty(`canvas:${id}`, false); };
  }, [id, projectId]);
  const dirty = (value: boolean, count?: number, fileCount?: number, providedSelection?: ReturnType<typeof canvasSelectionContext>) => {
    setDocumentDirty(`canvas:${id}`, value);
    const selection = providedSelection ?? canvasSelectionContext(api.current?.getSceneElements() ?? [], api.current?.getAppState().selectedElementIds ?? {});
    patchEditorState(editor.id, { title: latestTitle.current, dirty: value, saving: saving.current, imageFileCount: fileCount ?? Object.keys(api.current?.getFiles() ?? {}).length, revision: revision.current ?? 0, selectedObjectIds: JSON.stringify(selection.ids), selectedObjects: JSON.stringify(selection.objects), selectedObjectsTotal: selection.total, selectedObjectsTruncated: selection.truncated, selectionReady: Boolean(api.current), objectCount: count ?? api.current?.getSceneElements().length ?? 0 });
  };
  async function save() {
    const instance = api.current;
    if (!instance || saving.current) return false;
    if (instance.getSceneElements().some(element => element.type === "image" && (!element.fileId || !instance.getFiles()[element.fileId]))) { setError("图片仍在载入，请稍后保存"); return false; }
    saving.current = true; setBusy(true); setError("");
    const scene = serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
    const savedTitle = title.trim() || "未命名画布";
    dirty(true);
    try {
      revision.current = await saveCanvas({ id, projectId, revision: revision.current, title: savedTitle, scene, updatedAt: Date.now() });
      baseline.current = scene; titleRef.current = savedTitle;
      setTitle(savedTitle); latestTitle.current = savedTitle;
      saving.current = false;
      const current = serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
      dirty(current !== scene);
      return true;
    } catch (cause) { setError(`保存失败：${String(cause)}`); return false; }
    finally { saving.current = false; setBusy(false); if (api.current) { const current = serializeAsJSON(api.current.getSceneElements(), api.current.getAppState(), api.current.getFiles(), "local"); dirty(current !== baseline.current || latestTitle.current !== titleRef.current); } }
  }
  async function capturePreview() {
    const instance = api.current; if (!instance || !projectId || revision.current === undefined || saving.current) return false;
    const version = revision.current;
    const elements = instance.getSceneElements(); if (!elements.length) return false;
    const appState = instance.getAppState(), files = instance.getFiles();
    const observed = serializeAsJSON(elements, appState, files, "local");
    if (observed !== baseline.current) throw new Error("请先保存画布再生成预览。");
    setBusy(true);
    try {
      const render = (size: number) => exportToBlob({ elements, appState: { ...appState, exportWithDarkMode: false, exportEmbedScene: false, exportBackground: true }, files, mimeType: "image/png", maxWidthOrHeight: size });
      let blob = await render(1280);
      if (blob.size > 4.4 * 1024 * 1024) blob = await render(1024);
      const dataURL = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(blob); });
      if (version !== revision.current || serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== observed) throw new Error("生成预览时画布发生变化，请重新保存并交接。");
      await gatewayRequest("PUT", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(id)}/revisions/${version}/preview`, { dataURL });
      return true;
    } finally { setBusy(false); }
  }
  async function exportFile(format: "svg" | "png" | "excalidraw" | "sources") {
    const instance = api.current; if (!instance) return;
    setError("");
    try {
      const elements = structuredClone(instance.getSceneElements()), files = structuredClone(instance.getFiles());
      const state = instance.getAppState(), appState = { ...state, frameRendering: { ...state.frameRendering } };
      const scene = serializeAsJSON(elements, appState, files, "local"), exportTitle = title;
      if (format === "excalidraw") { download(new Blob([scene], { type: "application/json" }), `${exportTitle}.excalidraw`); return; }
      const manifest = await createSourceManifest({ scene, title: exportTitle, project_id: projectId, figure_id: id, based_on_revision: revision.current, unsaved_changes: scene !== baseline.current || exportTitle !== titleRef.current });
      if (format === "sources") download(new Blob([JSON.stringify(manifest, null, 2)], { type: "application/json" }), `${exportTitle}.sources.json`);
      else if (format === "svg") { const svg = await exportToSvg({ elements, appState: { ...appState, exportWithDarkMode: false }, files }); embedSvgSources(svg, manifest); download(new Blob([svg.outerHTML], { type: "image/svg+xml" }), `${exportTitle}.svg`); }
      else { const png = await exportToBlob({ elements, appState: { ...appState, exportWithDarkMode: false }, files, mimeType: "image/png" }); download(await embedPngSources(png, manifest), `${exportTitle}.png`); }
    } catch (cause) { setError(`导出失败：${String(cause)}`); }
  }
  const hasUnsavedChanges = () => {
    const instance = api.current;
    return saving.current || !instance || serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== baseline.current || latestTitle.current !== titleRef.current;
  };
  async function ensureSaved() {
    if (saving.current) return false;
    if (!hasUnsavedChanges()) return true;
    if (!await save()) return false;
    return !hasUnsavedChanges();
  }
  return <section aria-label="画布编辑器" className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDownCapture={event => {
    if (event.target instanceof HTMLElement && event.target.closest('[role="dialog"]')) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); event.stopPropagation(); void save(); }
  }}>
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b p-2">
      <input aria-label="画布名称" disabled={busy} className="min-w-48 flex-1 rounded border bg-background px-2 py-1 text-sm" value={title} onChange={event => { latestTitle.current = event.target.value; setTitle(event.target.value); dirty(true); }} />
      <span className="text-xs text-muted-foreground">{projectId ? "保存在项目中" : "旧版本地画布"}</span>
      {projectId && <CanvasHistory projectId={projectId} canvasId={id} getRevision={() => revision.current} isDirty={hasUnsavedChanges} onRestored={onReload} />}
      {projectId && <Button size="sm" variant={circleSelect ? "default" : "outline"} disabled={!loaded || busy} onClick={() => setCircleSelect(value => !value)}>{circleSelect ? "取消圈选" : "圈选批注"}</Button>}
      {projectId && <CanvasMarkups projectId={projectId} canvasId={id} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} />}
      {projectId && <CanvasPlanning projectId={projectId} canvasId={id} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onProposed={id => setProposalRequest({ id })} />}
      {projectId && <CanvasPalette projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onProposed={id => setProposalRequest({ id })} />}
      {projectId && <CanvasInset projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onProposed={id => setProposalRequest({ id })} />}
      {projectId && <CanvasRelation projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onProposed={id => setProposalRequest({ id })} />}
      {projectId && <CanvasRouting projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onProposed={id => setProposalRequest({ id })} />}
      {projectId && <CanvasProposals projectId={projectId} canvasId={id} isDirty={hasUnsavedChanges} onAccepted={onReload} request={proposalRequest} />}
      {projectId && <Button size="sm" variant="outline" onClick={() => setShowAgent(value => !value)}>Agent 绘图</Button>}
      <Button size="sm" variant="outline" onClick={() => setShowBioArt(value => !value)}>BioArt 素材</Button>
      <Button size="sm" variant="outline" onClick={() => setShowBioIcons(value => !value)}>矢量素材</Button>
      {projectId && <Button size="sm" variant="outline" disabled={!loaded || busy} onClick={() => {
        const instance = api.current;
        const changed = instance && (serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== baseline.current || latestTitle.current !== titleRef.current);
        if (!changed || window.confirm("重新加载将丢弃未保存修改，继续？")) onReload();
      }}>重新加载服务端版本</Button>}
      <Button size="sm" disabled={!loaded || busy || !imagesReady} onClick={() => void save()}>{busy ? "保存中…" : !imagesReady ? "图片载入中…" : "保存画布"}</Button>
      <Button size="sm" variant="outline" disabled={!loaded || !imagesReady} onClick={() => void exportFile("sources")}>导出素材来源</Button>
      {(["svg", "png", "excalidraw"] as const).map(format => <Button key={format} size="sm" variant="outline" disabled={!loaded || !imagesReady} onClick={() => void exportFile(format)}>导出 {format === "excalidraw" ? "源文件" : format.toUpperCase()}</Button>)}
    </div>
    {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
    {!loaded ? <p className="p-4 text-sm">{error ? "无法加载画布" : "加载画布…"}</p> : <div className="flex min-h-0 min-w-0 flex-1"><div className="relative min-h-0 min-w-0 flex-1" data-canvas-surface>
      <Excalidraw initialData={initial} excalidrawAPI={instance => { api.current = instance; }} langCode="zh-CN" handleKeyboardGlobally={false} autoFocus onChange={(elements, appState, files) => {
        if (!fitted.current && initial?.elements?.length && elements.length && api.current) {
          fitted.current = true;
          api.current.scrollToContent(elements, { fitToViewport: true, maxZoom: 1, viewportZoomFactor: 0.8 });
        }
        setImagesReady(elements.every(element => element.isDeleted || element.type !== "image" || Boolean(element.fileId && files[element.fileId])));
        const scene = serializeAsJSON(elements, appState, files, "local");
        if (baseline.current === null) { baseline.current = scene; dirty(false, elements.filter(element => !element.isDeleted).length, Object.keys(files).length); return; }
        const nextDirty = scene !== baseline.current || title !== titleRef.current;
        const previous = editor.state as { dirty?: boolean; objectCount?: number; selectedObjectIds?: string; selectedObjects?: string; selectedObjectsTotal?: number; selectedObjectsTruncated?: boolean; imageFileCount?: number } | null;
        const count = elements.filter(element => !element.isDeleted).length;
        const selection = canvasSelectionContext(elements, appState.selectedElementIds);
        const selectedObjectIds = JSON.stringify(selection.ids);
        const selectedObjects = JSON.stringify(selection.objects);
        if (previous?.dirty !== nextDirty || previous?.objectCount !== count || previous?.selectedObjectIds !== selectedObjectIds || previous?.selectedObjects !== selectedObjects || previous?.selectedObjectsTotal !== selection.total || previous?.selectedObjectsTruncated !== selection.truncated || previous?.imageFileCount !== Object.keys(files).length) dirty(nextDirty, count, Object.keys(files).length, selection);
      }}>
        <MainMenu><MainMenu.DefaultItems.LoadScene /><MainMenu.DefaultItems.Export /><MainMenu.DefaultItems.SaveAsImage /><MainMenu.DefaultItems.ClearCanvas /><MainMenu.DefaultItems.ChangeCanvasBackground /></MainMenu>
      </Excalidraw>
      {circleSelect && <CanvasCircleSelect getApi={() => api.current} onDone={(count, message) => {
        setCircleSelect(false); if (message) setError(message); else setError(""); if (count) setShowAgent(true);
      }} />}
    </div>{showAgent && projectId && <CanvasAgentPanel projectId={projectId} canvasId={id} getRevision={() => revision.current} getApi={() => api.current} capturePreview={capturePreview} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} />}{showBioArt && <BioArtPanel getApi={() => api.current} />}{showBioIcons && <BioIconsPanel getApi={() => api.current} />}</div>}
  </section>;
}
