import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { openEditor } from "@/app/workbench/workbench-store";
import { createCanvas, createProject, listProjects, refreshCanvases, useCanvases, type Project, selectedCanvasProject } from "./canvas-store";
import * as legacy from "./legacy-canvas-store";
export function CanvasSidebar() {
  const canvases = useCanvases();
  const localCanvases = legacy.useCanvases();
  useEffect(() => { void legacy.refreshCanvases().catch(() => {}); }, []);
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectId, setProjectId] = useState(() => selectedCanvasProject() ?? "");
  const [title, setTitle] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(true);
  useEffect(() => { void listProjects().then(items => { setProjects(items); setProjectId(current => items.some(item => item.id === current) ? current : items[0]?.id ?? ""); }).catch(cause => setError(String(cause))).finally(() => setBusy(false)); }, []);
  useEffect(() => { void refreshCanvases(projectId || undefined).catch(cause => setError(String(cause))); }, [projectId]);
  return <section aria-label="画布列表" className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto p-2">
    <label className="text-xs">可视化项目<select aria-label="可视化项目" className="mt-1 w-full rounded border bg-background p-1" value={projectId} onChange={event => setProjectId(event.target.value)}><option value="">选择项目</option>{projects.map(project => <option key={project.id} value={project.id}>{project.title}</option>)}</select></label>
    <form className="flex gap-1" onSubmit={async event => { event.preventDefault(); if (!title.trim() || busy) return; setBusy(true); setError(""); try { const project = await createProject(title.trim()); setProjects(items => [...items, project]); setProjectId(project.id); setTitle(""); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}><input aria-label="新项目名称" placeholder="新项目名称" className="min-w-0 flex-1 rounded border bg-background px-2 text-sm" value={title} onChange={event => setTitle(event.target.value)} /><Button size="sm" disabled={busy || !title.trim()} type="submit">创建项目</Button></form>
    <Button disabled={busy || !projectId} onClick={async () => {
      setBusy(true); setError("");
      try { const canvas = await createCanvas(projectId); openEditor({ kind: "canvas", params: { canvasId: canvas.id, projectId: canvas.projectId }, state: { title: canvas.title } }); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>新建画布</Button>
    <Button size="sm" variant="outline" disabled={busy || !projectId} onClick={async () => { setBusy(true); setError(""); try { await refreshCanvases(projectId); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>刷新画布列表</Button>
    {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    {canvases.map(canvas => <Button key={canvas.id} variant="ghost" className="justify-start" onClick={() => openEditor({ kind: "canvas", params: { canvasId: canvas.id, projectId: canvas.projectId }, state: { title: canvas.title } })}>{canvas.title}</Button>)}
    {!!localCanvases.length && <><p className="mt-3 text-xs text-muted-foreground">旧版本地画布：导出源文件后可导入项目画布</p>{localCanvases.map(canvas => <Button key={canvas.id} variant="ghost" className="justify-start" onClick={() => openEditor({ kind: "canvas", params: { canvasId: canvas.id }, state: { title: canvas.title } })}>{canvas.title}</Button>)}</>}
    {!canvases.length && <p className="p-2 text-xs text-muted-foreground">选择或创建项目，然后添加画布。</p>}
  </section>;
}
