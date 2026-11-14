import { useEffect, useState, type ReactElement, type FormEvent } from "react";
import { ChevronRight, FileImage, FolderOpen, FolderPlus, ImagePlus, LoaderCircle, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { allGroups, closeEditor, getGroup, getWorkbenchState, openEditor, patchEditorState, useWorkbenchState } from "@/app/workbench/workbench-store";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { cn } from "@/lib/utils";
import { createCanvas, createProject, deleteCanvas, listProjects, refreshCanvases, useCanvases, type CanvasRecord, type Project, selectedCanvasProject } from "./canvas-store";
import * as legacy from "./legacy-canvas-store";

const openCanvas = (canvas: CanvasRecord) => openEditor({ kind: "canvas", params: { canvasId: canvas.id, projectId: canvas.projectId }, state: { title: canvas.title } });

export function CanvasSidebar() {
  const canvases = useCanvases();
  const localCanvases = legacy.useCanvases();
  const workbench = useWorkbenchState();
  const group = getGroup(workbench.groups, workbench.groups.active);
  const active = group?.editors.find(editor => editor.id === group.activeId && editor.kind === "canvas");
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectId, setProjectId] = useState(() => selectedCanvasProject() ?? "");
  const [title, setTitle] = useState("");
  const [creating, setCreating] = useState(false);
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(true);
  const [loading, setLoading] = useState(true);
  useEffect(() => { void legacy.refreshCanvases().catch(() => {}); }, []);
  useEffect(() => {
    let current = true;
    void listProjects().then(items => {
      if (!current) return;
      setProjects(items);
      setProjectId(selected => items.some(item => item.id === selected) ? selected : items[0]?.id ?? "");
    }).catch(cause => { if (current) setError(String(cause)); }).finally(() => { if (current) setBusy(false); });
    return () => { current = false; };
  }, []);
  useEffect(() => {
    let current = true;
    setLoading(true); setQuery("");
    void refreshCanvases(projectId || undefined).catch(cause => { if (current) setError(String(cause)); }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [projectId]);

  async function addProject(event: FormEvent) {
    event.preventDefault();
    if (!title.trim() || busy) return;
    setBusy(true); setError("");
    try {
      const project = await createProject(title.trim());
      setProjects(items => [...items, project]); setProjectId(project.id); setTitle(""); setCreating(false);
      try { openCanvas(await createCanvas(project.id)); }
      catch (cause) { setError(`项目已创建，但首张画布创建失败，请点击“新建画布”重试。${String(cause)}`); }
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  async function addCanvas() {
    if (!projectId || busy) return;
    setBusy(true); setError("");
    try { openCanvas(await createCanvas(projectId)); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  async function refresh() {
    setLoading(true); setError("");
    try { await refreshCanvases(projectId || undefined); }
    catch (cause) { setError(String(cause)); }
    finally { setLoading(false); }
  }
  async function removeCanvas(canvas: CanvasRecord) {
    if (busy) return;
    const matchingEditors = () => allGroups(getWorkbenchState().groups).flatMap(([, group]) => group.editors)
      .filter(editor => editor.kind === "canvas" && editor.params.canvasId === canvas.id && editor.params.projectId === canvas.projectId);
    if (matchingEditors().some(editor => (editor.state as { saving?: boolean } | null)?.saving)) {
      setError("画布正在保存，请稍后再删除。"); return;
    }
    if (!window.confirm(`确定删除画布“${canvas.title}”？此操作无法撤销，未保存的修改也会丢弃。`)) return;
    setBusy(true); setError("");
    try {
      await deleteCanvas(canvas);
      for (const editor of matchingEditors()) {
        patchEditorState(editor.id, { ...(editor.state as object), dirty: false, saving: false });
        closeEditor(editor.id);
      }
    } catch (cause) { setError(`删除画布失败：${String(cause)}`); }
    finally { setBusy(false); }
  }
  const canvasMenu = (canvas: CanvasRecord, trigger: ReactElement) => <ContextMenu key={canvas.id}>
    <ContextMenuTrigger render={<div />}>{trigger}</ContextMenuTrigger>
    <ContextMenuContent><ContextMenuItem variant="destructive" disabled={busy} onClick={() => void removeCanvas(canvas)}><Trash2 />删除画布</ContextMenuItem></ContextMenuContent>
  </ContextMenu>;
  const showCreate = creating || (!busy && projects.length === 0);
  const visibleCanvases = canvases.filter(canvas => canvas.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  return <section aria-label="画布列表" className="flex min-h-0 flex-1 flex-col overflow-hidden">
    <div className="shrink-0 space-y-2 border-b p-3">
      <div className="flex items-center gap-1">
        <FolderOpen className="mr-1 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <select aria-label="可视化项目" title="切换项目" className="h-8 min-w-0 flex-1 rounded-md border-0 bg-transparent px-1 text-sm font-medium outline-none focus-visible:ring-2 focus-visible:ring-ring" disabled={busy} value={projectId} onChange={event => { setProjectId(event.target.value); setError(""); setCreating(false); }}>
          <option value="">{projects.length ? "选择项目" : "尚无项目"}</option>
          {projects.map(project => <option key={project.id} value={project.id}>{project.title}</option>)}
        </select>
        <Button size="icon-xs" variant="ghost" aria-label="新建项目" title="新建项目" disabled={busy} onClick={() => setCreating(value => !value)}><FolderPlus /></Button>
      </div>
      {showCreate && <form onSubmit={event => void addProject(event)} className="space-y-2 rounded-lg border bg-background/50 p-3">
        <div className="flex items-center justify-between"><span className="text-xs font-medium">{projects.length ? "新建项目" : "创建第一个项目"}</span>{projects.length > 0 && <Button size="icon-xs" variant="ghost" aria-label="取消新建项目" onClick={() => setCreating(false)}><X /></Button>}</div>
        <input autoFocus aria-label="新项目名称" placeholder="项目名称，例如：细胞信号通路" className="h-8 w-full min-w-0 rounded-md border bg-background px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring" value={title} onChange={event => setTitle(event.target.value)} onKeyDown={event => { if (event.key === "Escape" && projects.length) setCreating(false); }} disabled={busy} />
        <Button size="sm" className="w-full" disabled={busy || !title.trim()} type="submit">{busy ? <LoaderCircle className="animate-spin" /> : <Plus />}创建项目</Button>
        {!projects.length && <p className="text-xs leading-relaxed text-muted-foreground">创建后，自动为你打开第一张画布。</p>}
      </form>}
    </div>
    {error && <p role="alert" className="mx-3 mt-3 shrink-0 rounded-md border border-destructive/25 bg-destructive/5 p-2 text-xs leading-relaxed text-destructive">{error}</p>}
    <div className="flex h-11 shrink-0 items-center gap-1 px-3">
      <span className="text-xs font-medium text-muted-foreground">画布</span>
      {canvases.length > 0 && <span className="text-[11px] tabular-nums text-muted-foreground/70">{canvases.length}</span>}
      <span className="flex-1" />
      <Button size="xs" variant="ghost" aria-label="新建画布" title="新建画布" disabled={busy || !projectId} onClick={() => void addCanvas()}><Plus />新建画布</Button>
      <Button size="icon-xs" variant="ghost" aria-label="刷新画布列表" title="刷新" disabled={busy || loading || !projectId} onClick={() => void refresh()}><RefreshCw className={loading ? "animate-spin" : ""} /></Button>
    </div>
    {canvases.length > 5 && <label className="mx-3 mb-2 flex h-8 shrink-0 items-center gap-2 rounded-md border bg-background/50 px-2"><Search className="size-3.5 text-muted-foreground" aria-hidden="true" /><input aria-label="搜索画布" placeholder="搜索画布…" className="w-full min-w-0 bg-transparent text-xs outline-none" value={query} onChange={event => setQuery(event.target.value)} /></label>}
    <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
      {loading ? <p role="status" className="flex items-center justify-center gap-2 p-6 text-xs text-muted-foreground"><LoaderCircle className="size-3.5 animate-spin" />正在加载画布…</p> : visibleCanvases.length ? <div className="space-y-1">{visibleCanvases.map(canvas => {
        const selected = active?.params.canvasId === canvas.id && active?.params.projectId === canvas.projectId;
        return canvasMenu(canvas, <Button variant="ghost" aria-label={canvas.title} aria-current={selected ? "page" : undefined} title={canvas.title} onClick={() => openCanvas(canvas)} className={cn("group h-auto w-full justify-start gap-2.5 whitespace-normal rounded-md border border-transparent px-2.5 py-2 text-left", selected && "border-primary/15 bg-primary/10 text-primary")}>
          <FileImage className={cn("size-4 shrink-0 text-muted-foreground", selected && "text-primary")} aria-hidden="true" />
          <span className="min-w-0 flex-1"><span className="block truncate text-sm">{canvas.title}</span><span className="mt-0.5 block text-[11px] text-muted-foreground">{Number.isFinite(canvas.updatedAt) ? new Date(canvas.updatedAt).toLocaleDateString("zh-CN") : ""}</span></span>
          <ChevronRight className="size-3.5 shrink-0 opacity-0 group-hover:opacity-60" aria-hidden="true" />
        </Button>);
      })}</div> : <div className="flex flex-col items-center px-4 py-9 text-center">
        <ImagePlus className="mb-3 size-7 text-muted-foreground/40" aria-hidden="true" />
        <p className="text-sm text-muted-foreground">{query ? "没有匹配的画布" : projectId ? "还没有画布" : "用项目整理你的科研图稿"}</p>
        {projectId && !query && <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground/70">点击上方「新建画布」开始绘图</p>}
      </div>}
      {localCanvases.length > 0 && <details className="mt-4 border-t pt-3"><summary className="cursor-pointer px-2 text-xs text-muted-foreground">旧版本地画布 · {localCanvases.length}</summary><p className="px-2 py-2 text-xs text-muted-foreground">导出源文件后，可导入项目画布。</p>{localCanvases.map(canvas => canvasMenu(canvas, <Button variant="ghost" className="w-full justify-start truncate" onClick={() => openCanvas(canvas)}>{canvas.title}</Button>))}</details>}
    </div>
  </section>;
}
