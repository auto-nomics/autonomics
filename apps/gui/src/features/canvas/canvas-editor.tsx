import { CanvasCodeDrawing } from "./canvas-code-drawing";
import { CanvasLayers } from "./canvas-layers";
import { isLayerHidden } from "./canvas-layers-model";
import { CanvasMaterialBrowser } from "./canvas-material-browser";
import { CanvasBottomActions } from "./canvas-bottom-actions";
import { Download, Save, MessageSquare, Shapes, LayoutGrid, History, Layers } from "lucide-react";
import { CanvasActionMenu, CanvasActionGroup } from "./canvas-action-menu";
import { normalizeEmptyNativeFields } from "./canvas-empty-native-fields";
import { CanvasFrameSwap } from "./canvas-frame-swap";
import { SvgPathChannel } from "./canvas-svg-path-channel";
import { emptySvgPathState, svgPathTransition, type SvgPathEvent, type SvgPathContext } from "./canvas-svg-path-context";
import { embedLocalSvgFonts } from "./canvas-svg-fonts";
import { hasProtectedCopyTarget, prepareNativePaste, repairNativeDuplicate, stampNativeIdentities } from "./canvas-native-duplicate";
import { useCallback, useEffect, useRef, useState } from "react";
import { Excalidraw, MainMenu, CaptureUpdateAction, exportToBlob, exportToSvg, serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import "@excalidraw/excalidraw/index.css";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { findEditor, patchEditorState } from "@/app/workbench/workbench-store";
import type { EditorContext } from "@/app/workbench/editor-registry";
import { canvasSelectionContext } from "./canvas-selection-context";
import { createSourceManifest, embedSvgSources, embedPngSources } from "./canvas-source-manifest";
import { CanvasInset } from "./canvas-inset";
import { CanvasPalette } from "./canvas-palette";
import { CanvasTextEdit } from "./canvas-text-edit";
import { CanvasEvidenceList } from "./canvas-evidence-list";
import { CanvasDuplicate } from "./canvas-duplicate";
import { CanvasRelation } from "./canvas-relation";
import { CanvasFrameFit } from "./canvas-frame-fit";
import { CanvasRouting } from "./canvas-routing";
import { CanvasMarkups } from "./canvas-markups";
import { CanvasHistory } from "./canvas-history";
import { CanvasIllustratorContext } from "./canvas-illustrator-context";
import { useCanvasIllustrator } from "./use-canvas-illustrator";
import { CanvasSelectionAnnotations } from "./canvas-selection-annotations";
import { preserveSourceFiles } from "./canvas-source-files";
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
  const surface = useRef<HTMLDivElement>(null);
  const baseline = useRef<string | null>(null);
  const sourceFiles = useRef<Record<string, unknown>>({});
  const serializeScene = (...args: Parameters<typeof serializeAsJSON>) => preserveSourceFiles(serializeAsJSON(...args), sourceFiles.current);
  const pointerSubscription=useRef<(()=>void)|undefined>(undefined);
  const stopProtectedPointer=useRef<(()=>void)|undefined>(undefined);
  useEffect(()=>()=>{pointerSubscription.current?.();stopProtectedPointer.current?.();},[]);
  const fitted = useRef(false);
  const viewportSize = useRef<{width:number;height:number}|null>(null);
  const titleRef = useRef("");
  const saving = useRef(false);
  const syncing = useRef(false);
  const previewRevision = useRef<number | undefined>(undefined);
  const lastEditAt = useRef(0);
  const syncRef = useRef<() => Promise<void>>(async () => {});
  const latestTitle = useRef("未命名画布");
  const [title, setTitle] = useState("未命名画布");
  const [error, setError] = useState("");
  const [imagesReady, setImagesReady] = useState(true);
  const [busy, setBusy] = useState(false);
  const [initial, setInitial] = useState<ExcalidrawInitialDataState | null>(null);
  const [loaded, setLoaded] = useState(false);
  const illustrator = useCanvasIllustrator(editor.id, projectId, id, loaded);
  const pathState = useRef(emptySvgPathState);
  const pathDirty = useRef(false);
  const [inspectedPath, setInspectedPath] = useState<SvgPathContext | null>(null);
  const priorSelection = useRef<string | undefined>(undefined);
  const publishPath = useCallback((event: SvgPathEvent) => {
    if (event.kind === "inspect" && (event.snapshot.project_id !== projectId || event.snapshot.figure_id !== id || pathDirty.current || event.snapshot.expected_revision !== revision.current)) event = {kind:"clear"};
    const next = svgPathTransition(pathState.current,event);
    if (next === pathState.current) return;
    pathState.current = next; setInspectedPath(next.snapshot);
    const latest=findEditor(editor.id)?.state;
    patchEditorState(editor.id,{...(latest && typeof latest === "object" ? latest : {}),inspectedSvgPath:next.snapshot?JSON.stringify(next.snapshot):null});
  },[editor.id,id,projectId]);
  useEffect(() => {
    let cancelled = false;
    pathDirty.current=false; priorSelection.current=undefined; publishPath({kind:"clear"});
    patchEditorState(editor.id, { selectedObjectIds: "[]", selectedObjects: "[]", selectedObjectsTotal: 0, selectedObjectsTruncated: false, selectionReady: false, inspectedSvgPath: null });
    void readCanvas(id, projectId).then(record => {
      if (cancelled) return;
      revision.current = record.revision;
      titleRef.current = record.title; latestTitle.current = record.title; setTitle(record.title);
      sourceFiles.current = record.scene ? JSON.parse(record.scene).files ?? {} : {};
      setInitial(normalizeEmptyNativeFields(record.scene ? { ...JSON.parse(record.scene), scrollToContent: true } : { appState: { currentItemRoughness: 0, currentItemFontFamily: 9, currentItemRoundness: "sharp", viewBackgroundColor: "#ffffff" } }));
      setLoaded(true);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; setDocumentDirty(`canvas:${id}`, false); };
  }, [id, projectId]);
  const dirty = (value: boolean, count?: number, fileCount?: number, providedSelection?: ReturnType<typeof canvasSelectionContext>) => {
    setDocumentDirty(`canvas:${id}`, value);
    if (value && !saving.current) lastEditAt.current = Date.now();
    const selection = providedSelection ?? canvasSelectionContext(api.current?.getSceneElements() ?? [], api.current?.getAppState().selectedElementIds ?? {});
    pathDirty.current=value;
    const selectionKey=JSON.stringify(selection.ids);
    publishPath({kind:"scene",revision:revision.current,dirty:value,selectionChanged:priorSelection.current!==undefined&&priorSelection.current!==selectionKey,selectedIds:selection.ids});
    priorSelection.current=selectionKey;
    const currentState = findEditor(editor.id)?.state;
    patchEditorState(editor.id, { ...(currentState && typeof currentState === "object" ? currentState : {}), inspectedSvgPath: pathState.current.snapshot ? JSON.stringify(pathState.current.snapshot) : null, title: latestTitle.current, dirty: value, saving: saving.current, imageFileCount: fileCount ?? Object.keys(api.current?.getFiles() ?? {}).length, revision: revision.current ?? 0, selectedObjectIds: JSON.stringify(selection.ids), selectedObjects: JSON.stringify(selection.objects), selectedObjectsTotal: selection.total, selectedObjectsTruncated: selection.truncated, selectionReady: Boolean(api.current), objectCount: count ?? api.current?.getSceneElements().length ?? 0 });
  };
  async function save() {
    const instance = api.current;
    if (!instance || saving.current) return false;
    if (instance.getSceneElements().some(element => element.type === "image" && (!element.fileId || !instance.getFiles()[element.fileId]))) { setError("图片仍在载入，请稍后保存"); return false; }
    saving.current = true; setBusy(true); setError("");
    const scene = serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
    const savedTitle = title.trim() || "未命名画布";
    dirty(true);
    try {
      revision.current = await saveCanvas({ id, projectId, revision: revision.current, title: savedTitle, scene, updatedAt: Date.now() });
      baseline.current = scene; titleRef.current = savedTitle;
      setTitle(savedTitle); latestTitle.current = savedTitle;
      const current = serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
      dirty(current !== scene);
      if (projectId && current === scene && instance.getSceneElements().some(element => !element.isDeleted)) {
        try { await capturePreview(true); }
        catch (cause) { setError(`画布已保存，但预览更新失败：${String(cause)}`); }
      }
      return true;
    } catch (cause) { setError(`保存失败：${String(cause)}`); return false; }
    finally { saving.current = false; setBusy(false); if (api.current) { const current = serializeScene(api.current.getSceneElements(), api.current.getAppState(), api.current.getFiles(), "local"); dirty(current !== baseline.current || latestTitle.current !== titleRef.current); } }
  }
  async function capturePreview(allowWhileSaving = false) {
    const instance = api.current; if (!instance || !projectId || revision.current === undefined || (saving.current && !allowWhileSaving)) return false;
    const version = revision.current;
    const elements = instance.getSceneElements(); if (!elements.length) return false;
    const appState = instance.getAppState(), files = instance.getFiles();
    const observed = serializeScene(elements, appState, files, "local");
    if (observed !== baseline.current) throw new Error("请先保存画布再生成预览。");
    setBusy(true);
    try {
      const render = (size: number) => exportToBlob({ elements: elements.filter(element => !isLayerHidden(element)), appState: { ...appState, exportWithDarkMode: false, exportEmbedScene: false, exportBackground: true }, files, mimeType: "image/png", maxWidthOrHeight: size });
      let blob = await render(1280);
      if (blob.size > 4.4 * 1024 * 1024) blob = await render(1024);
      const dataURL = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(blob); });
      if (version !== revision.current || serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== observed) throw new Error("生成预览时画布发生变化，请重新保存并交接。");
      await gatewayRequest("PUT", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(id)}/revisions/${version}/preview`, { dataURL });
      previewRevision.current = version;
      return true;
    } finally { setBusy(false); }
  }
  async function exportFile(format: "svg" | "png" | "excalidraw" | "sources") {
    const instance = api.current; if (!instance) return;
    setError("");
    try {
      const state = instance.getAppState(), appState = { ...state, frameRendering: { ...state.frameRendering } };
      // Export the serialized snapshot: runtime provenance and renderer caches stay local.
      const scene = serializeScene(instance.getSceneElements(), appState, instance.getFiles(), "local"), exportTitle = title;
      const snapshot = JSON.parse(scene) as {elements: ReturnType<ExcalidrawImperativeAPI["getSceneElements"]>;files: ReturnType<ExcalidrawImperativeAPI["getFiles"]>};
      const {elements,files} = snapshot;
      if (format === "excalidraw") { download(new Blob([scene], { type: "application/json" }), `${exportTitle}.excalidraw`); return; }
      const manifest = await createSourceManifest({ scene, title: exportTitle, project_id: projectId, figure_id: id, based_on_revision: revision.current, unsaved_changes: scene !== baseline.current || exportTitle !== titleRef.current });
      if (format === "sources") download(new Blob([JSON.stringify(manifest, null, 2)], { type: "application/json" }), `${exportTitle}.sources.json`);
      else if (format === "svg") { const svg = await exportToSvg({ elements: elements.filter(element => !isLayerHidden(element)), appState: { ...appState, exportWithDarkMode: false }, files }); await embedLocalSvgFonts(svg); embedSvgSources(svg, manifest); download(new Blob([svg.outerHTML], { type: "image/svg+xml" }), `${exportTitle}.svg`); }
      else { const png = await exportToBlob({ elements: elements.filter(element => !isLayerHidden(element)), appState: { ...appState, exportWithDarkMode: false }, files, mimeType: "image/png" }); download(await embedPngSources(png, manifest), `${exportTitle}.png`); }
    } catch (cause) { setError(`导出失败：${String(cause)}`); }
  }
  const hasUnsavedChanges = () => {
    const instance = api.current;
    return saving.current || !instance || serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== baseline.current || latestTitle.current !== titleRef.current;
  };
  async function syncCanvas() {
    const instance = api.current;
    if (!projectId || !instance || !loaded || saving.current || syncing.current) return;
    syncing.current = true;
    try {
      const head = await gatewayRequest("GET", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(id)}/head`) as { revision: number };
      if (head.revision === revision.current) {
        if (hasUnsavedChanges()) {
          if (Date.now() - lastEditAt.current >= 600) await save();
        } else if (previewRevision.current !== revision.current && instance.getSceneElements().length) {
          await capturePreview();
        }
        return;
      }
      const record = await readCanvas(id, projectId);
      if (hasUnsavedChanges()) {
        setError("Agent 已更新画布；本地有未保存的编辑，请先导出本地源文件，再重新载入最新画布。");
        return;
      }
      const scene = normalizeEmptyNativeFields(JSON.parse(record.scene || "{}"));
      sourceFiles.current = structuredClone(scene.files ?? {});
      revision.current = record.revision;
      titleRef.current = record.title; latestTitle.current = record.title; setTitle(record.title);
      instance.addFiles(Object.values(scene.files ?? {}));
      instance.updateScene({ elements: scene.elements, ...(typeof scene.appState?.viewBackgroundColor === "string" ? { appState: { viewBackgroundColor: scene.appState.viewBackgroundColor } } : {}), captureUpdate: CaptureUpdateAction.IMMEDIATELY });
      baseline.current = serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
      dirty(false);
      setError("");
      if (instance.getSceneElements().length) await capturePreview();
    } catch (cause) { setError(`画布同步失败：${String(cause)}`); }
    finally { syncing.current = false; }
  }
  syncRef.current = syncCanvas;
  useEffect(() => {
    if (!projectId || !loaded) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => { await syncRef.current(); if (!stopped) timer = setTimeout(tick, 750); };
    void tick();
    return () => { stopped = true; clearTimeout(timer); };
  }, [projectId, id, loaded]);
  async function ensureSaved() {
    if (saving.current) return false;
    if (!hasUnsavedChanges()) return true;
    if (!await save()) return false;
    return !hasUnsavedChanges();
  }
  return <CanvasIllustratorContext.Provider value={illustrator.path}><SvgPathChannel.Provider value={publishPath}><section aria-label="画布编辑器" className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDownCapture={event => {
    if (event.target instanceof HTMLElement && event.target.closest('[role="dialog"]')) return;
    const key=event.key.toLowerCase(), target=event.target;
    const history=(event.ctrlKey||event.metaKey)&&!event.altKey&&(key==="z"||(key==="y"&&!event.shiftKey));
    if(history&&target instanceof HTMLElement&&!target.closest('input,textarea,select,[contenteditable]:not([contenteditable="false"]),[role="textbox"]')) {
      const canvas=event.currentTarget.querySelector<HTMLElement>('[data-canvas-surface] .excalidraw');
      if(canvas&&(!canvas.contains(target)||key==="y")) {
        event.preventDefault();event.stopPropagation();canvas.focus({preventScroll:true});
        // Normalize the redo alias; native Ctrl+Y is otherwise Windows-only.
        canvas.dispatchEvent(new KeyboardEvent("keydown",{key:key==="y"?"Z":event.key,code:key==="y"?"KeyZ":event.code,ctrlKey:event.ctrlKey,metaKey:event.metaKey,shiftKey:key==="y"||event.shiftKey,altKey:event.altKey,repeat:event.repeat,bubbles:true,cancelable:true}));
        return;
      }
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); event.stopPropagation(); void save(); }
  }}>
    {inspectedPath && <div aria-label="SVG 路径上下文" className="flex items-center gap-2 border-b px-2 py-1 text-xs text-muted-foreground"><span>{inspectedPath.panel_visible?"正在查看":"上次查看"}：画布 SVG 路径 {inspectedPath.part_index}</span><Button size="sm" variant="ghost" onClick={() => publishPath({kind:"clear"})}>清除路径上下文</Button></div>}
    {illustrator.error && <p role="alert" className="p-2 text-sm text-destructive">{illustrator.error}</p>}
    {error && <div role="alert" className="p-2 text-sm text-destructive">{error}<Button variant="ghost" size="sm" onClick={() => { if (!hasUnsavedChanges() || window.confirm("重新载入会丢弃本地未保存的编辑。请先导出需要保留的内容，是否继续？")) onReload(); }}>重新载入画布</Button></div>}
    {!loaded ? <p className="p-4 text-sm">{error ? "无法加载画布" : "加载画布…"}</p> : <div className="flex min-h-0 min-w-0 flex-1"><div className="relative min-h-0 min-w-0 flex-1" style={{ containerType: "inline-size" }} ref={surface} data-canvas-surface>
      <Excalidraw initialData={initial} excalidrawAPI={instance => {
        api.current = instance;pointerSubscription.current?.();
        pointerSubscription.current=instance.onPointerDown((_tool,pointerState,event)=>{
          stopProtectedPointer.current?.();
          if(_tool.type!=="selection"||event.button!==0)return;
          const pointerId=event.pointerId;
          const block=(move:PointerEvent)=>{
            if(move.pointerId!==pointerId||!move.altKey)return;
            const state=instance.getAppState();
            if(pointerState.resize.handleType||state.croppingElementId||state.editingLinearElement||(!pointerState.hit.element&&!pointerState.hit.hasHitCommonBoundingBoxOfSelectedElements))return;
            if(!hasProtectedCopyTarget(instance.getSceneElements(),state.selectedElementIds,pointerState.hit.element?.id))return;
            move.preventDefault();move.stopImmediatePropagation();
            setError("放大图和批注暂不支持 Alt 拖拽复制；可松开 Alt 移动原对象。");
          };
          const cleanup=()=>{window.removeEventListener("pointermove",block,true);window.removeEventListener("pointerup",cleanup,true);window.removeEventListener("pointercancel",cleanup,true);window.removeEventListener("blur",cleanup);if(stopProtectedPointer.current===cleanup)stopProtectedPointer.current=undefined;};
          stopProtectedPointer.current=cleanup;
          window.addEventListener("pointermove",block,{capture:true,passive:false});window.addEventListener("pointerup",cleanup,{capture:true,once:true});window.addEventListener("pointercancel",cleanup,{capture:true,once:true});window.addEventListener("blur",cleanup,{once:true});
        });
      }} langCode="zh-CN" handleKeyboardGlobally={false} autoFocus onPaste={data=>{
        if(!data.elements)return true;
        try {const prepared=prepareNativePaste(data.elements,data.files,api.current?.getFiles()??{});data.elements=prepared.elements;data.files=prepared.files;return true;}
        catch(cause){setError(String(cause));return false;}
      }} onDuplicate={(next,previous) => {
        const selected=api.current?.getAppState().selectedElementIds;
        try {return repairNativeDuplicate(next,previous);} catch(cause) {setError(String(cause));queueMicrotask(()=>{if(selected)api.current?.updateScene({appState:{selectedElementIds:selected}});});return [...previous];}
      }} onChange={(elements, appState, files) => {
        stampNativeIdentities(elements);
        if (!fitted.current && initial?.elements?.length && elements.length && api.current) {
          fitted.current = true;
          api.current.scrollToContent(elements, { fitToViewport: true, maxZoom: 1, viewportZoomFactor: 0.8 });
        }
        // Preserve the scene point at the viewport center, without resetting user zoom/pan.
        if (appState.width > 0 && appState.height > 0) {
          const previousSize=viewportSize.current;
          viewportSize.current={width:appState.width,height:appState.height};
          if(previousSize && api.current && (previousSize.width!==appState.width || previousSize.height!==appState.height)) {
            const zoom=appState.zoom.value;
            if(Number.isFinite(zoom)&&zoom>0)api.current.updateScene({appState:{scrollX:appState.scrollX+(appState.width-previousSize.width)/(2*zoom),scrollY:appState.scrollY+(appState.height-previousSize.height)/(2*zoom)}});
          }
        }
        setImagesReady(elements.every(element => element.isDeleted || element.type !== "image" || Boolean(element.fileId && files[element.fileId])));
        const scene = serializeScene(elements, appState, files, "local");
        if (baseline.current === null) { baseline.current = scene; dirty(false, elements.filter(element => !element.isDeleted).length, Object.keys(files).length); return; }
        const nextDirty = scene !== baseline.current || title !== titleRef.current;
        const previous = editor.state as { dirty?: boolean; objectCount?: number; selectedObjectIds?: string; selectedObjects?: string; selectedObjectsTotal?: number; selectedObjectsTruncated?: boolean; imageFileCount?: number } | null;
        const count = elements.filter(element => !element.isDeleted).length;
        const selection = canvasSelectionContext(elements, appState.selectedElementIds);
        const selectedObjectIds = JSON.stringify(selection.ids);
        const selectedObjects = JSON.stringify(selection.objects);
        if (previous?.dirty !== nextDirty || previous?.objectCount !== count || previous?.selectedObjectIds !== selectedObjectIds || previous?.selectedObjects !== selectedObjects || previous?.selectedObjectsTotal !== selection.total || previous?.selectedObjectsTruncated !== selection.truncated || previous?.imageFileCount !== Object.keys(files).length) dirty(nextDirty, count, Object.keys(files).length, selection);
      }}>
        <MainMenu>{projectId && <MainMenu.Item onSelect={illustrator.openConversation}>{illustrator.path ? "打开插画专家对话" : "连接插画专家"}</MainMenu.Item>}<MainMenu.Item onSelect={() => {
          if (busy) return;
          const next = window.prompt("重命名画布", latestTitle.current);
          if (next === null) return;
          const name = next.trim();
          if (!name) { setError("画布名称不能为空。"); return; }
          if (name === latestTitle.current) return;
          latestTitle.current = name; setTitle(name); dirty(true); setError("");
        }}>重命名</MainMenu.Item><MainMenu.Separator /><MainMenu.DefaultItems.LoadScene /><MainMenu.DefaultItems.Export /><MainMenu.DefaultItems.SaveAsImage /><MainMenu.DefaultItems.ClearCanvas /><MainMenu.DefaultItems.ChangeCanvasBackground /></MainMenu>
      <CanvasMaterialBrowser getApi={() => api.current} />
      <CanvasLayers getApi={() => api.current} />
      <CanvasBottomActions surface={surface}>
        <div data-canvas-action-dock role="toolbar" aria-label="画布快捷操作" className="relative flex max-w-full flex-wrap items-center justify-center gap-0.5 px-1" onPointerDown={event => event.stopPropagation()}>
    <div data-canvas-toolbar role="group" aria-label="画布工具栏" className="flex min-w-0 items-center gap-1">
      <Button size="sm" variant="ghost" disabled={!loaded || busy || !imagesReady} onClick={() => void save()}><Save aria-hidden="true" />{busy ? "保存中…" : !imagesReady ? "图片载入中…" : "保存画布"}</Button>
    </div>
          <Button size="sm" variant="ghost" onClick={() => api.current?.toggleSidebar({ name: "layers" })}><Layers />图层</Button>
          <CanvasActionMenu label="审阅批注" icon={MessageSquare}>
            <CanvasActionGroup title="审阅与协作">
      {projectId && <CanvasMarkups agentPath={illustrator.path} projectId={projectId} canvasId={id} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} capturePreview={capturePreview} isDirty={hasUnsavedChanges} />}
            </CanvasActionGroup>
          </CanvasActionMenu>
          <CanvasActionMenu label="对象编辑" icon={Shapes}>
            <CanvasActionGroup title="对象编辑">
      <CanvasCodeDrawing getApi={() => api.current} disabled={!loaded || busy} />
      {projectId && <CanvasPalette capturePreview={capturePreview} projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasInset projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasRelation projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasDuplicate projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasEvidenceList projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasTextEdit projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
            </CanvasActionGroup>
          </CanvasActionMenu>
          <CanvasActionMenu label="布局连线" icon={LayoutGrid}>
            <CanvasActionGroup title="分面与连线">
      {projectId && <CanvasRouting projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasFrameFit projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
      {projectId && <CanvasFrameSwap projectId={projectId} canvasId={id} disabled={!loaded || busy} getApi={() => api.current} getRevision={() => revision.current} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} onApplied={() => void syncCanvas()} />}
            </CanvasActionGroup>
          </CanvasActionMenu>
          <CanvasActionMenu label="版本" icon={History}>
            <CanvasActionGroup title="版本操作">
      {projectId && <CanvasHistory projectId={projectId} canvasId={id} getRevision={() => revision.current} isDirty={hasUnsavedChanges} onRestored={onReload} />}
      {projectId && <Button size="sm" variant="outline" disabled={!loaded || busy} onClick={() => {
        const instance = api.current;
        const changed = instance && (serializeScene(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local") !== baseline.current || latestTitle.current !== titleRef.current);
        if (!changed || window.confirm("重新加载将丢弃未保存修改，继续？")) onReload();
      }}>重新加载服务端版本</Button>}
            </CanvasActionGroup>
          </CanvasActionMenu>
          <CanvasActionMenu label="导出" icon={Download}><CanvasActionGroup title="导出图稿与来源">
      <Button size="sm" variant="outline" disabled={!loaded || !imagesReady} onClick={() => void exportFile("sources")}>导出素材来源</Button>
      {(["svg", "png", "excalidraw"] as const).map(format => <Button key={format} size="sm" variant="outline" disabled={!loaded || !imagesReady} onClick={() => void exportFile(format)}>导出 {format === "excalidraw" ? "源文件" : format.toUpperCase()}</Button>)}
          </CanvasActionGroup></CanvasActionMenu>
        </div>
      </CanvasBottomActions>
      </Excalidraw>
      {projectId && <CanvasSelectionAnnotations surface={surface} agentPath={illustrator.path} projectId={projectId} canvasId={id} getRevision={() => revision.current} getApi={() => api.current} capturePreview={capturePreview} ensureSaved={ensureSaved} isDirty={hasUnsavedChanges} />}
    </div></div>}

  </section></SvgPathChannel.Provider></CanvasIllustratorContext.Provider>;
}
