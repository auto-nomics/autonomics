import { useCallback, useEffect, useState } from "react";
import { FileCode2, FolderOpen, RefreshCw, Save, TerminalSquare } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { patchEditorState, type EditorRef } from "@/app/workbench/workbench-store";

type Project = { name: string; status: string | null; image: string | null; error: string | null };
type Environment = { id: string; reference: string };

export function ProjectEditor({ editor }: { editor: EditorRef }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [project, setProject] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [path, setPath] = useState("");
  const [contents, setContents] = useState("");
  const [original, setOriginal] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [environment, setEnvironment] = useState("");
  const [purpose, setPurpose] = useState("");
  const dirty = contents !== original;
  useEffect(() => { patchEditorState(editor.id, { dirty }); }, [editor.id, dirty]);

  const refresh = useCallback(async () => {
    try {
      const [list, env] = await Promise.all([gatewayRequest("GET", "/plugin-projects"), gatewayRequest("GET", "/plugins/environments")]);
      setProjects(list.projects); setEnvironments(env.environments);
      setEnvironment((current) => current || env.environments[0]?.id || "");
      setError("");
    } catch (cause) { setError(String(cause)); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!project) { setFiles([]); return; }
    let cancelled = false;
    gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/files`)
      .then((result) => { if (!cancelled) setFiles(result.files); })
      .catch((cause) => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; };
  }, [project]);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => { if (dirty) event.preventDefault(); };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  const mayLeave = () => !dirty || window.confirm("当前文件有未保存的修改，是否放弃？");
  const selectProject = (next: string) => {
    if (!mayLeave()) return;
    setProject(next); setPath(""); setContents(""); setOriginal(""); setFiles([]); setError("");
  };
  const openFile = async (next: string) => {
    if (next === path || !mayLeave()) return;
    setBusy(true); setError("");
    try {
      const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(next)}`);
      setPath(next); setContents(result.contents); setOriginal(result.contents);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const save = async () => {
    if (!path || !dirty || busy) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("PUT", `/plugin-projects/${encodeURIComponent(project)}/file`, { path, contents, original });
      setOriginal(contents);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const create = async (event: React.FormEvent) => {
    event.preventDefault(); if (!mayLeave()) return; setBusy(true); setError("");
    try {
      await gatewayRequest("POST", "/plugin-projects", { name, environment_id: environment, purpose });
      await refresh(); setProject(name); setPath(""); setContents(""); setOriginal(""); setFiles([]); setCreating(false); setName(""); setPurpose("");
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const selected = projects.find((item) => item.name === project);
  return <section aria-label="插件项目" className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDown={(event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); }
  }}>
    <header className="flex flex-wrap items-center gap-2 border-b px-4 py-3">
      <FolderOpen className="size-4 text-primary"/><h1 className="mr-auto font-semibold">插件项目</h1>
      <Button size="sm" variant="outline" onClick={() => void refresh()}><RefreshCw /> 刷新</Button>
      <Button size="sm" onClick={() => setCreating(!creating)}>新建项目</Button>
    </header>
    {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
    {creating && <form onSubmit={(event) => void create(event)} className="grid gap-2 border-b bg-muted/30 p-4 sm:grid-cols-2">
      <label className="grid gap-1 text-xs">项目名称<Input required pattern="[a-z][a-z0-9-]*[a-z0-9]|[a-z]" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-plugin" /></label>
      <label className="grid gap-1 text-xs">开发环境<select required className="h-9 rounded-md border bg-background px-2 text-sm" value={environment} onChange={(event) => setEnvironment(event.target.value)}><option value="">选择已批准的环境</option>{environments.map((item) => <option key={item.id} value={item.id}>{item.id}</option>)}</select></label>
      <label className="grid gap-1 text-xs sm:col-span-2">项目用途<Input required value={purpose} onChange={(event) => setPurpose(event.target.value)} /></label>
      <div className="flex gap-2 sm:col-span-2"><Button disabled={busy || !environment} size="sm" type="submit">创建</Button><Button size="sm" type="button" variant="ghost" onClick={() => setCreating(false)}>取消</Button></div>
    </form>}
    <div className="flex min-h-0 flex-1 flex-col md:flex-row">
      <aside className="flex max-h-48 w-full shrink-0 flex-col overflow-auto border-b bg-muted/20 md:max-h-none md:w-64 md:border-b-0 md:border-r">
        <div className="px-3 pt-3 text-xs font-semibold uppercase text-muted-foreground">项目</div>
        {projects.length === 0 && <p className="p-3 text-xs text-muted-foreground">暂无项目</p>}
        {projects.map((item) => <button type="button" key={item.name} disabled={busy} onClick={() => selectProject(item.name)} className={`px-3 py-2 text-left text-sm hover:bg-accent ${project === item.name ? "bg-accent font-medium" : ""}`}><span className="block truncate">{item.name}</span><span className="block truncate text-xs text-muted-foreground">{item.error ? "需要修复" : item.status}</span></button>)}
        {project && <><div className="border-t px-3 pt-3 text-xs font-semibold uppercase text-muted-foreground">文件</div>{files.map((file) => <button type="button" key={file} disabled={busy} onClick={() => void openFile(file)} className={`flex items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-accent ${path === file ? "bg-accent" : ""}`}><FileCode2 className="size-3.5 shrink-0"/><span className="break-all">{file}</span></button>)}</>}
      </aside>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {selected?.error && <div role="status" className="border-b bg-amber-500/10 p-3 text-xs">项目 manifest 需要修复：{selected.error}</div>}
        {path ? <><div className="flex items-center gap-2 border-b px-4 py-2 text-sm"><span className="min-w-0 flex-1 truncate" title={path}>{path}{dirty ? " •" : ""}</span><Button size="sm" variant="outline" disabled={!dirty || busy} onClick={() => void save()}><Save /> 保存</Button></div><textarea aria-label={`编辑 ${path}`} className="min-h-48 w-full flex-1 resize-none bg-background p-4 font-mono text-sm leading-6 outline-none" spellCheck={false} value={contents} onChange={(event) => setContents(event.target.value)} /></> : <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-muted-foreground">{project ? "从左侧选择文件开始编辑" : "选择或创建一个插件项目"}</div>}
        <div className="border-t px-4 py-3 text-xs text-muted-foreground"><div className="flex items-center gap-2 font-medium text-foreground"><TerminalSquare className="size-4"/>终端</div><p className="mt-1">容器会话和交互式终端尚未接入。</p></div>
      </div>
    </div>
  </section>;
}
