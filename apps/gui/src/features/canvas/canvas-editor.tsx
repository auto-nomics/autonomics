import { useEffect, useRef, useState } from "react";
import { Excalidraw, MainMenu, exportToBlob, exportToSvg, serializeAsJSON } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import "@excalidraw/excalidraw/index.css";
import { Button } from "@/components/ui/button";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";
import { patchEditorState } from "@/app/workbench/workbench-store";
import type { EditorContext } from "@/app/workbench/editor-registry";
import { readCanvas, saveCanvas } from "./canvas-store";

function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a"); link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
export function CanvasEditor({ editor }: EditorContext) {
  const id = String(editor.params.canvasId);
  const api = useRef<ExcalidrawImperativeAPI | null>(null);
  const baseline = useRef<string | null>(null);
  const titleRef = useRef("");
  const saving = useRef(false);
  const latestTitle = useRef("未命名画布");
  const [title, setTitle] = useState("未命名画布");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [initial, setInitial] = useState<ExcalidrawInitialDataState | null>(null);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    let cancelled = false;
    void readCanvas(id).then(record => {
      if (cancelled) return;
      titleRef.current = record.title; latestTitle.current = record.title; setTitle(record.title);
      setInitial(record.scene ? { ...JSON.parse(record.scene), scrollToContent: true } : { appState: { currentItemRoughness: 0, currentItemFontFamily: 2, currentItemRoundness: "sharp", viewBackgroundColor: "#ffffff" } });
      setLoaded(true);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; setDocumentDirty(`canvas:${id}`, false); };
  }, [id]);
  const dirty = (value: boolean, count?: number) => {
    setDocumentDirty(`canvas:${id}`, value);
    patchEditorState(editor.id, { title: latestTitle.current, dirty: value, saving: saving.current, objectCount: count ?? api.current?.getSceneElements().length ?? 0 });
  };
  async function save() {
    const instance = api.current;
    if (!instance || saving.current) return;
    saving.current = true; setBusy(true); setError("");
    const scene = serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
    const savedTitle = title.trim() || "未命名画布";
    dirty(true);
    try {
      await saveCanvas({ id, title: savedTitle, scene, updatedAt: Date.now() });
      baseline.current = scene; titleRef.current = savedTitle;
      setTitle(savedTitle); latestTitle.current = savedTitle;
      saving.current = false;
      const current = serializeAsJSON(instance.getSceneElements(), instance.getAppState(), instance.getFiles(), "local");
      dirty(current !== scene);
    } catch (cause) { setError(`保存失败：${String(cause)}`); }
    finally { saving.current = false; setBusy(false); if (api.current) { const current = serializeAsJSON(api.current.getSceneElements(), api.current.getAppState(), api.current.getFiles(), "local"); dirty(current !== baseline.current || latestTitle.current !== titleRef.current); } }
  }
  async function exportFile(format: "svg" | "png" | "excalidraw") {
    const instance = api.current; if (!instance) return;
    setError("");
    try {
      const elements = instance.getSceneElements(), appState = instance.getAppState(), files = instance.getFiles();
      if (format === "excalidraw") download(new Blob([serializeAsJSON(elements, appState, files, "local")], { type: "application/json" }), `${title}.excalidraw`);
      else if (format === "svg") { const svg = await exportToSvg({ elements, appState: { ...appState, exportWithDarkMode: false }, files }); download(new Blob([svg.outerHTML], { type: "image/svg+xml" }), `${title}.svg`); }
      else download(await exportToBlob({ elements, appState: { ...appState, exportWithDarkMode: false }, files, mimeType: "image/png" }), `${title}.png`);
    } catch (cause) { setError(`导出失败：${String(cause)}`); }
  }
  return <section aria-label="画布编辑器" className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDownCapture={event => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); event.stopPropagation(); void save(); }
  }}>
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b p-2">
      <input aria-label="画布名称" disabled={busy} className="min-w-0 flex-1 rounded border bg-background px-2 py-1 text-sm" value={title} onChange={event => { latestTitle.current = event.target.value; setTitle(event.target.value); dirty(true); }} />
      <span className="text-xs text-muted-foreground">保存在当前设备</span>
      <Button size="sm" disabled={!loaded || busy} onClick={() => void save()}>{busy ? "保存中…" : "保存画布"}</Button>
      {(["svg", "png", "excalidraw"] as const).map(format => <Button key={format} size="sm" variant="outline" disabled={!loaded} onClick={() => void exportFile(format)}>导出 {format === "excalidraw" ? "源文件" : format.toUpperCase()}</Button>)}
    </div>
    {error && <p role="alert" className="p-2 text-sm text-destructive">{error}</p>}
    {!loaded ? <p className="p-4 text-sm">{error ? "无法加载画布" : "加载画布…"}</p> : <div className="relative min-h-0 min-w-0 flex-1" data-canvas-surface>
      <Excalidraw initialData={initial} excalidrawAPI={instance => { api.current = instance; }} langCode="zh-CN" handleKeyboardGlobally={false} autoFocus onChange={(elements, appState, files) => {
        const scene = serializeAsJSON(elements, appState, files, "local");
        if (baseline.current === null) { baseline.current = scene; dirty(false, elements.filter(element => !element.isDeleted).length); return; }
        const nextDirty = scene !== baseline.current || title !== titleRef.current;
        const previous = editor.state as { dirty?: boolean; objectCount?: number } | null;
        const count = elements.filter(element => !element.isDeleted).length;
        if (previous?.dirty !== nextDirty || previous?.objectCount !== count) dirty(nextDirty, count);
      }}>
        <MainMenu><MainMenu.DefaultItems.LoadScene /><MainMenu.DefaultItems.Export /><MainMenu.DefaultItems.SaveAsImage /><MainMenu.DefaultItems.ClearCanvas /><MainMenu.DefaultItems.ChangeCanvasBackground /></MainMenu>
      </Excalidraw>
    </div>}
  </section>;
}
