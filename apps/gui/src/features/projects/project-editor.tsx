import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useProjectSidebarSlot } from "./sidebar-slot";
import { ProjectFileTree } from "./project-file-tree";
import { CodeEditor } from "./code-editor";
import { DebugClient } from "./debug-client";
import { DebugInspector } from "./debug-inspector";
import { applyTextEdits, type FileEdits } from "./workspace-edit";
import { publishProjectChanges, type ProjectFileChange } from "./watch-events";
import { FileCode2, FilePlus2, Folder, FolderPlus, PanelLeftOpen, Pencil, RefreshCw, Save, TerminalSquare, Trash2, X } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getGroup, patchEditorState, setContainer, setPanelTab, setPanelVisible, setSidebarCollapsed, useWorkbenchState, type EditorRef } from "@/app/workbench/workbench-store";

type Project = { name: string; status: string | null; image: string | null; error: string | null };
type Environment = { id: string; reference: string };
type Buffer = { contents: string; original: string };

export function ProjectEditor({ editor }: { editor: EditorRef }) {
  const slot = useProjectSidebarSlot();
  const workbench = useWorkbenchState();
  const group = getGroup(workbench.groups, workbench.groups.active);
  const active = group?.activeId === editor.id;
  const [sidebarTab, setSidebarTab] = useState<"projects" | "files">("projects");
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [filesRevision, setFilesRevision] = useState(0);
  const [saveVersion, setSaveVersion] = useState(0);
  const [projects, setProjects] = useState<Project[]>([]);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [project, setProject] = useState("");
  const [connected, setConnected] = useState(false);
  const [vscodeFrames, setVscodeFrames] = useState<Record<string, string>>({});
  const [vscodeDirty, setVscodeDirty] = useState<Record<string, boolean>>({});
  const vscodeUrl = vscodeFrames[project] ?? "";
  const [showVscode, setShowVscode] = useState(false);
  const [containerVersion, setContainerVersion] = useState(0);
  const [connecting, setConnecting] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [directories, setDirectories] = useState<string[]>([]);
  const [path, setPath] = useState("");
  const [treeSelection, setTreeSelection] = useState<{ path: string; kind: "file" | "directory" }>();
  const [directoryMove, setDirectoryMove] = useState<{ from: string; to: string; id: number }>();
  const [navigation, setNavigation] = useState<{ path: string; line: number; column: number; id: number }>();
  const [contents, setContents] = useState("");
  const [original, setOriginal] = useState("");
  const [buffers, setBuffers] = useState<Record<string, Buffer>>({});
  const [error, setError] = useState("");
  const [watchError, setWatchError] = useState("");
  const [conflicts, setConflicts] = useState<Record<string, "changed" | "deleted">>({});
  const [busy, setBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [environment, setEnvironment] = useState("");
  const [purpose, setPurpose] = useState("");
  const [breakpoints, setBreakpoints] = useState<Record<string, number[]>>({});
  const [debugStatus, setDebugStatus] = useState("");
  const [debugOutput, setDebugOutput] = useState("");
  const [debugThread, setDebugThread] = useState<number>();
  const [debugStop, setDebugStop] = useState(0);
  const debugRef = useRef<DebugClient | null>(null);
  const autoShowVscode = useRef(true);
  const workspaceEditLock = useRef(false);
  const editRevision = useRef(0);
  const workspaceState = useRef({ project, path, connected, busy, buffers });
  workspaceState.current = { project, path, connected, busy, buffers };
  const dirty = contents !== original;
  const localDirty = dirty || Object.values(buffers).some(buffer => buffer.contents !== buffer.original);
  const anyDirty = localDirty || Object.values(vscodeDirty).some(Boolean);
  useEffect(() => { if (active) setContainer("projects"); }, [active]);
  useEffect(() => { patchEditorState(editor.id, { dirty: anyDirty, project, containerVersion }); }, [editor.id, anyDirty, project, containerVersion]);
  useEffect(() => () => { debugRef.current?.dispose(); debugRef.current = null; }, [project]);

  const toggleBreakpoint = (line: number) => {
    if (!path.endsWith(".py")) return;
    const lines = breakpoints[path] ?? [];
    const next = lines.includes(line) ? lines.filter(item => item !== line) : [...lines, line].sort((a, b) => a - b);
    setBreakpoints(current => ({ ...current, [path]: next }));
    if (debugRef.current) void debugRef.current.setBreakpoints(path, next).catch(cause => setDebugStatus(String(cause)));
  };
  const startDebug = async () => {
    if (!connected || !path.endsWith(".py") || dirty || debugRef.current) return;
    setDebugOutput(""); setDebugStatus("正在启动容器内调试器…"); setDebugThread(undefined);
    const client = new DebugClient(project, message => {
      if (message.event === "output") setDebugOutput(current => (current + String(message.body?.output ?? "")).slice(-30000));
      if (message.event === "stopped") {
        const threadId = message.body?.threadId;
        setDebugThread(threadId); setDebugStop(value => value + 1); setDebugStatus(`已暂停：${message.body?.reason ?? "断点"}`);
      }
      if (message.event === "continued") { setDebugThread(undefined); setDebugStatus("运行中"); }
      if (message.event === "terminated") { setDebugThread(undefined); setDebugStatus("调试已结束"); client.dispose(); if (debugRef.current === client) debugRef.current = null; }
    }, cause => { setDebugStatus(`调试失败：${cause.message}`); if (debugRef.current === client) debugRef.current = null; });
    debugRef.current = client;
    try { await client.start(path, breakpoints); if (debugRef.current === client) setDebugStatus(current => current.startsWith("已暂停") ? current : "运行中"); }
    catch (cause) { if (debugRef.current === client) { setDebugStatus(`调试失败：${String(cause)}`); debugRef.current = null; client.dispose(); } }
  };
  const debugCommand = (command: string) => { if (debugRef.current && debugThread !== undefined) void debugRef.current.request(command, { threadId: debugThread }).catch(cause => setDebugStatus(String(cause))); };
  const stopDebug = () => { const client = debugRef.current; debugRef.current = null; setDebugThread(undefined); setDebugStatus("调试已停止"); if (client) void client.request("disconnect", { terminateDebuggee: true }).catch(() => {}).finally(() => client.dispose()); };

  const refresh = useCallback(async () => {
    try {
      const [list, env] = await Promise.all([gatewayRequest("GET", "/plugin-projects"), gatewayRequest("GET", "/plugins/environments")]);
      setProjects(list.projects); setEnvironments(env.environments);
      setEnvironment((current) => current || env.environments[0]?.id || "");
      setFilesRevision(value => value + 1);
      setError("");
    } catch (cause) { setError(String(cause)); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => { autoShowVscode.current = true; setShowVscode(false); }, [project]);
  useEffect(() => {
    if (!project || !connected) return;
    let cancelled = false;
    void gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/vscode-session`).then(result => {
      if (cancelled || typeof result?.url !== "string") return;
      try {
        const url = new URL(result.url);
        if (url.protocol !== "http:" || url.hostname !== "127.0.0.2" || !url.port) return;
        setVscodeFrames(current => ({ ...current, [project]: url.href }));
        if (autoShowVscode.current && !workspaceState.current.path) setShowVscode(true);
      } catch { /* An invalid workbench URL is never embedded. */ }
    }).catch(() => { /* The optional VS Code service may not be configured. */ });
    return () => { cancelled = true; };
  }, [project, connected, containerVersion]);
  useEffect(() => {
    const projects = Object.keys(vscodeFrames);
    if (!projects.length) return;
    let cancelled = false, timer = 0;
    const poll = async () => {
      await Promise.all(projects.map(async item => {
        try {
          const result = await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(item)}/vscode-control`, { action: "state" });
          if (!cancelled && result?.ready === true && typeof result.dirty === "boolean") {
            setVscodeDirty(current => current[item] === result.dirty ? current : { ...current, [item]: result.dirty });
          }
        } catch { /* Preserve the last known state while the extension host reconnects. */ }
      }));
      if (!cancelled) timer = window.setTimeout(() => void poll(), 2000);
    };
    void poll();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [vscodeFrames]);
  useEffect(() => {
    if (!project) return;
    let cancelled = false;
    const check = async () => {
      try {
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/container`);
        if (!cancelled) setConnected(result.connected === true);
      } catch { if (!cancelled) setConnected(false); }
    };
    const timer = window.setInterval(() => void check(), 5000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [project]);
  useEffect(() => {
    if (!project || !connected) { setFiles([]); setDirectories([]); setLoadingFiles(false); return; }
    let cancelled = false;
    setLoadingFiles(true);
    gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/files`)
      .then((result) => { if (!cancelled) { setFiles(result.files); setDirectories(result.directories ?? []); } })
      .catch((cause) => { if (!cancelled) setError(String(cause)); })
      .finally(() => { if (!cancelled) setLoadingFiles(false); });
    return () => { cancelled = true; };
  }, [project, connected, filesRevision]);
  useEffect(() => {
    if (!project || !connected) { setWatchError(""); return; }
    type Entry = { path: string; kind: "file" | "directory"; stamp: string };
    let cancelled = false, timer = 0;
    let previous: Map<string, Entry> | undefined;
    const pending = new Map<string, 1 | 2 | 3>();
    const clearConflict = (file: string) => setConflicts(current => { if (!current[file]) return current; const next = { ...current }; delete next[file]; return next; });
    const tick = async () => {
      try {
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/snapshot`);
        if (cancelled) return;
        if (!Array.isArray(result.entries)) throw new Error("文件快照格式无效");
        const next = new Map<string, Entry>();
        for (const entry of result.entries as Entry[]) {
          if (!entry || typeof entry.path !== "string" || !["file", "directory"].includes(entry.kind) || typeof entry.stamp !== "string") throw new Error("文件快照格式无效");
          next.set(entry.path, entry);
        }
        const changes: ProjectFileChange[] = [];
        let treeChanged = false;
        if (previous) {
          for (const [file, entry] of previous) if (!next.has(file) || next.get(file)?.kind !== entry.kind) { treeChanged = true; changes.push({ path: file, type: 3 }); }
          for (const [file, entry] of next) {
            const old = previous.get(file);
            if (!old || old.kind !== entry.kind) { treeChanged = true; changes.push({ path: file, type: 1 }); }
            else if (entry.kind === "file" && old.stamp !== entry.stamp) changes.push({ path: file, type: 2 });
          }
        } else {
          for (const file of Object.keys(workspaceState.current.buffers)) if (!file.split("/").some(part => [".git", ".venv", "node_modules"].includes(part))) pending.set(file, next.has(file) ? 2 : 3);
        }
        previous = next;
        if (treeChanged) setFilesRevision(value => value + 1);
        for (const change of changes) if (workspaceState.current.buffers[change.path]) pending.set(change.path, next.get(change.path)?.kind === "file" ? (change.type === 1 ? 1 : 2) : 3);
        publishProjectChanges(project, changes);
        setWatchError("");
        for (const [file, kind] of [...pending].slice(0, 20)) {
          const state = workspaceState.current;
          if (cancelled || !state.connected || state.project !== project || state.busy) break;
          const buffer = state.buffers[file];
          if (!buffer) { pending.delete(file); clearConflict(file); continue; }
          if (kind === 3) { pending.delete(file); setConflicts(current => ({ ...current, [file]: "deleted" })); continue; }
          const revision = editRevision.current;
          try {
            const fileResult = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(file)}`);
            if (cancelled) return;
            const latest = workspaceState.current;
            if (latest.project !== project || !latest.connected || latest.busy || latest.buffers[file] !== buffer || revision !== editRevision.current) continue;
            if (typeof fileResult.contents !== "string") throw new Error("文件内容无效");
            pending.delete(file);
            if (fileResult.contents === buffer.original) { clearConflict(file); continue; }
            if (buffer.contents !== buffer.original) { setConflicts(current => ({ ...current, [file]: "changed" })); continue; }
            const updated = { contents: fileResult.contents, original: fileResult.contents };
            setBuffers(current => current[file] === buffer ? { ...current, [file]: updated } : current);
            if (latest.path === file) { setContents(updated.contents); setOriginal(updated.original); }
            clearConflict(file);
          } catch { /* Retry on the next snapshot; the file may be mid-write. */ }
        }
      } catch (cause) { if (!cancelled) setWatchError(`文件监控不可用：${String(cause)}`); }
      if (!cancelled) timer = window.setTimeout(() => void tick(), 3000);
    };
    void tick();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [project, connected]);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => { if (anyDirty) event.preventDefault(); };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [anyDirty]);

  const mayLeave = () => !localDirty || window.confirm("项目中有未保存的修改，是否放弃？");
  const selectProject = async (next: string, rebuild = false) => {
    if (next !== project && !mayLeave()) return;
    if (rebuild && (localDirty || vscodeDirty[project]) && !window.confirm("重建容器会关闭工作台，请先保存未保存的修改。是否继续？")) return;
    setBusy(true); setConnecting(next); setError("");
    if (rebuild) setConnected(false);
    try {
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(next)}/container${rebuild ? "?rebuild=true" : ""}`);
      if (rebuild) setContainerVersion(value => value + 1);
      setConnected(true);
      setFilesRevision(value => value + 1);
      setSidebarTab("files");
      if (next !== project) { setProject(next); setBuffers({}); setConflicts({}); setBreakpoints({}); setDebugStatus(""); setDebugOutput(""); setDebugThread(undefined); setPath(""); setTreeSelection(undefined); setDirectoryMove(undefined); setNavigation(undefined); setContents(""); setOriginal(""); setFiles([]); setDirectories([]); }
    } catch (cause) {
      if (next === project) {
        try { const status = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(next)}/container`); setConnected(status.connected === true); }
        catch { setConnected(false); }
      }
      setError(`无法连接开发容器：${String(cause)}`);
    }
    finally { setBusy(false); setConnecting(""); }
  };
  const openFile = async (next: string, target?: { line: number; column: number }, preferVscode = false): Promise<boolean> => {
    if (!connected) return false;
    const cachedDirty = buffers[next] && buffers[next].contents !== buffers[next].original;
    if (preferVscode && vscodeUrl && !cachedDirty) {
      setBusy(true); setError("");
      try {
        const result = await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/vscode-control`, { action: "open", path: next, ...target });
        if (result?.opened === true) { setTreeSelection({ path: next, kind: "file" }); setShowVscode(true); return true; }
      } catch (cause) { setError(`无法在 VS Code 中打开文件：${String(cause)}`); return false; }
      finally { setBusy(false); }
    }
    autoShowVscode.current = false;
    setShowVscode(false);
    if (next === path) { setTreeSelection({ path: next, kind: "file" }); if (target) setNavigation(current => ({ path: next, ...target, id: (current?.id ?? 0) + 1 })); return true; }
    const cached = buffers[next];
    if (cached) { setPath(next); setContents(cached.contents); setOriginal(cached.original); setTreeSelection({ path: next, kind: "file" }); setNavigation(current => target ? { path: next, ...target, id: (current?.id ?? 0) + 1 } : undefined); return true; }
    setBusy(true); setError("");
    try {
      const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(next)}`);
      setBuffers(current => ({ ...current, [next]: { contents: result.contents, original: result.contents } }));
      setPath(next); setContents(result.contents); setOriginal(result.contents);
      setTreeSelection({ path: next, kind: "file" });
      setNavigation(current => target ? { path: next, ...target, id: (current?.id ?? 0) + 1 } : undefined);
      return true;
    } catch (cause) { setError(String(cause)); return false; }
    finally { setBusy(false); }
  };
  const save = async () => {
    if (!connected || !path || !dirty || busy) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("PUT", `/plugin-projects/${encodeURIComponent(project)}/file`, { path, contents, original });
      setOriginal(contents);
      setBuffers(current => ({ ...current, [path]: { contents, original: contents } }));
      setConflicts(current => { const next = { ...current }; delete next[path]; return next; });
      setSaveVersion(version => version + 1);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const reloadFile = async () => {
    if (!connected || busy || !path || (dirty && !window.confirm("重新读取会放弃当前文件的未保存修改，是否继续？"))) return;
    setBusy(true); setError("");
    try {
      const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/file?path=${encodeURIComponent(path)}`);
      setContents(result.contents); setOriginal(result.contents);
      setBuffers(current => ({ ...current, [path]: { contents: result.contents, original: result.contents } }));
      setConflicts(current => { const next = { ...current }; delete next[path]; return next; });
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const changeContents = (value: string) => { editRevision.current++; setContents(value); setBuffers(current => ({ ...current, [path]: { contents: value, original: current[path]?.original ?? original } })); };
  const applyWorkspaceEdits = async (edits: FileEdits[], valid: () => boolean) => {
    const snapshot = workspaceState.current;
    if (!snapshot.connected || snapshot.busy || workspaceEditLock.current || !valid()) throw new Error("项目不可编辑或语言服务修改已过期");
    if (edits.some(edit => edit.path !== snapshot.path && snapshot.buffers[edit.path] && snapshot.buffers[edit.path].contents !== snapshot.buffers[edit.path].original)) throw new Error("请先保存其他已修改文件，再执行跨文件操作");
    const revision = editRevision.current;
    workspaceEditLock.current = true; setBusy(true);
    try {
      const loaded = await Promise.all(edits.map(async edit => {
        const buffer = snapshot.buffers[edit.path];
        if (buffer) return buffer;
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(snapshot.project)}/file?path=${encodeURIComponent(edit.path)}`);
        if (typeof result.contents !== "string") throw new Error(`无法读取 ${edit.path}`);
        return { contents: result.contents, original: result.contents };
      }));
      if (!valid() || revision !== editRevision.current || workspaceState.current.project !== snapshot.project || workspaceState.current.path !== snapshot.path || workspaceState.current.buffers !== snapshot.buffers || !workspaceState.current.connected) throw new Error("项目内容已更改，请重试");
      const next = { ...snapshot.buffers };
      edits.forEach((edit, index) => { next[edit.path] = { contents: applyTextEdits(loaded[index].contents, edit.edits), original: loaded[index].original }; });
      setBuffers(next);
      if (next[snapshot.path]) setContents(next[snapshot.path].contents);
      return edits.some(edit => edit.path === snapshot.path) ? next[snapshot.path]?.contents : undefined;
    } finally { workspaceEditLock.current = false; setBusy(false); }
  };
  const closeBuffer = (file: string) => {
    const buffer = buffers[file];
    if (!buffer || (buffer.contents !== buffer.original && !window.confirm(`文件 ${file} 有未保存的修改，是否放弃？`))) return;
    const remaining = Object.keys(buffers).filter(item => item !== file);
    setBuffers(current => { const next = { ...current }; delete next[file]; return next; });
    setConflicts(current => { const next = { ...current }; delete next[file]; return next; });
    if (file === path) {
      const next = remaining.at(-1);
      setPath(next ?? ""); setContents(next ? buffers[next].contents : ""); setOriginal(next ? buffers[next].original : "");
      setTreeSelection(next ? { path: next, kind: "file" } : undefined); setNavigation(undefined);
    }
  };
  const createFile = async () => {
    if (!project || !connected || busy) return;
    const next = window.prompt("输入项目内的新文件路径（父目录必须已存在）", treeSelection?.kind === "directory" ? `${treeSelection.path}/` : "");
    if (next === null || !next.trim()) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/file`, { path: next });
      setFilesRevision(value => value + 1);
      await openFile(next);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const createDirectory = async () => {
    if (!project || !connected || busy) return;
    const next = window.prompt("输入项目内的新文件夹路径（父目录必须已存在）", treeSelection?.kind === "directory" ? `${treeSelection.path}/` : "");
    if (next === null || !next.trim()) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/directory`, { path: next });
      setTreeSelection({ path: next, kind: "directory" });
      setFilesRevision(value => value + 1);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const renameFile = async () => {
    if (!project || !treeSelection || !connected || busy) return;
    const previous = treeSelection.path;
    const isDirectory = treeSelection.kind === "directory";
    const next = window.prompt(`输入新的项目内${isDirectory ? "文件夹" : "文件"}路径`, previous);
    if (next === null || !next.trim() || next === previous) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/${isDirectory ? "directory" : "file"}/rename`, { path: previous, new_path: next });
      setBuffers(current => Object.fromEntries(Object.entries(current).map(([file, buffer]) => [file === previous || (isDirectory && file.startsWith(`${previous}/`)) ? next + file.slice(previous.length) : file, buffer])));
      setConflicts(current => Object.fromEntries(Object.entries(current).map(([file, conflict]) => [file === previous || (isDirectory && file.startsWith(`${previous}/`)) ? next + file.slice(previous.length) : file, conflict])));
      setBreakpoints(current => Object.fromEntries(Object.entries(current).map(([file, lines]) => [file === previous || (isDirectory && file.startsWith(`${previous}/`)) ? next + file.slice(previous.length) : file, lines])));
      if (path === previous || (isDirectory && path.startsWith(`${previous}/`))) setPath(next + path.slice(previous.length));
      if (isDirectory) setDirectoryMove(current => ({ from: previous, to: next, id: (current?.id ?? 0) + 1 }));
      setTreeSelection({ path: next, kind: treeSelection.kind }); setNavigation(undefined); setFilesRevision(value => value + 1);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const deleteFile = async () => {
    if (!project || !treeSelection || !connected || busy) return;
    const selectedPath = treeSelection.path;
    const isDirectory = treeSelection.kind === "directory";
    const affects = (file: string) => file === selectedPath || (isDirectory && file.startsWith(`${selectedPath}/`));
    const affectsOpen = affects(path);
    const losesChanges = Object.entries(buffers).some(([file, buffer]) => affects(file) && buffer.contents !== buffer.original);
    if (!window.confirm(`确定删除${isDirectory ? "文件夹及其全部内容" : "文件"} ${selectedPath}？${losesChanges ? "未保存的修改也会丢失。" : ""}`)) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("DELETE", `/plugin-projects/${encodeURIComponent(project)}/${isDirectory ? "directory" : "file"}?path=${encodeURIComponent(selectedPath)}`);
      const remaining = Object.entries(buffers).filter(([file]) => !affects(file));
      setBuffers(Object.fromEntries(remaining));
      setConflicts(current => Object.fromEntries(Object.entries(current).filter(([file]) => !affects(file))));
      setBreakpoints(current => Object.fromEntries(Object.entries(current).filter(([file]) => file !== selectedPath && !(isDirectory && file.startsWith(`${selectedPath}/`)))));
      if (affectsOpen) { const next = remaining.at(-1); setPath(next?.[0] ?? ""); setNavigation(undefined); setContents(next?.[1].contents ?? ""); setOriginal(next?.[1].original ?? ""); }
      setTreeSelection(undefined); setFilesRevision(value => value + 1);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const create = async (event: React.FormEvent) => {
    event.preventDefault(); if (!mayLeave()) return; setBusy(true); setError("");
    try {
      await gatewayRequest("POST", "/plugin-projects", { name, environment_id: environment, purpose });
      await refresh(); setCreating(false); setName(""); setPurpose("");
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(name)}/container`);
      setConnected(true);
      setSidebarTab("files"); setProject(name); setBuffers({}); setConflicts({}); setBreakpoints({}); setDebugStatus(""); setDebugOutput(""); setDebugThread(undefined); setPath(""); setTreeSelection(undefined); setDirectoryMove(undefined); setNavigation(undefined); setContents(""); setOriginal(""); setFiles([]); setDirectories([]);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const selected = projects.find((item) => item.name === project);
  return <section aria-label="插件项目" className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDown={(event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); }
  }}>
    {(workbench.sidebar.collapsed || workbench.sidebar.container !== "projects") && <div className="flex shrink-0 border-b px-2 py-1">
      <Button size="sm" variant="ghost" onClick={() => { setContainer("projects"); setSidebarCollapsed(false); }}><PanelLeftOpen />项目与文件</Button>
    </div>}
    {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
    {watchError && <div role="status" className="border-b px-4 py-2 text-xs text-muted-foreground">{watchError}</div>}
    {connecting && <div role="status" className="border-b px-4 py-2 text-xs text-muted-foreground">正在连接 {connecting} 的开发容器…</div>}
    {project && !connected && <div role="status" className="flex items-center gap-2 border-b bg-amber-500/10 px-4 py-2 text-xs">开发容器已断开，文件暂为只读。<Button size="xs" variant="outline" disabled={busy} onClick={() => void selectProject(project)}>重新连接</Button></div>}
    {creating && <form onSubmit={(event) => void create(event)} className="grid gap-2 border-b bg-muted/30 p-4 sm:grid-cols-2">
      <label className="grid gap-1 text-xs">项目名称<Input required pattern="[a-z][a-z0-9-]*[a-z0-9]|[a-z]" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-plugin" /></label>
      <label className="grid gap-1 text-xs">开发环境<select required className="h-9 rounded-md border bg-background px-2 text-sm" value={environment} onChange={(event) => setEnvironment(event.target.value)}><option value="">选择已批准的环境</option>{environments.map((item) => <option key={item.id} value={item.id}>{item.id}</option>)}</select></label>
      <label className="grid gap-1 text-xs sm:col-span-2">项目用途<Input required value={purpose} onChange={(event) => setPurpose(event.target.value)} /></label>
      <div className="flex gap-2 sm:col-span-2"><Button disabled={busy || !environment} size="sm" type="submit">创建</Button><Button size="sm" type="button" variant="ghost" onClick={() => setCreating(false)}>取消</Button></div>
    </form>}
    {active && slot && createPortal(
      <aside aria-label="插件项目导航" className="flex min-h-0 flex-1 flex-col">
        <div className="flex h-9 shrink-0 items-center gap-0.5 border-b px-1">
        <div role="tablist" aria-label="插件项目侧栏" className="flex min-w-0 items-center gap-0.5">
          {([
            { id: "projects", label: "项目", icon: Folder },
            { id: "files", label: "文件", icon: FileCode2 },
          ] as const).map(({ id, label, icon: Icon }) => <button
            key={id} id={`project-sidebar-tab-${id}`} type="button" role="tab"
            aria-selected={sidebarTab === id} aria-controls={`project-sidebar-panel-${id}`}
            tabIndex={sidebarTab === id ? 0 : -1}
            className={`flex h-7 items-center gap-1 rounded-md px-1 text-xs ${sidebarTab === id ? "bg-secondary text-secondary-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"}`}
            onClick={() => setSidebarTab(id)}
            onKeyDown={(event) => {
              if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
              event.preventDefault();
              const next = id === "projects" ? "files" : "projects";
              setSidebarTab(next);
              document.getElementById(`project-sidebar-tab-${next}`)?.focus();
            }}
          ><Icon className="size-3.5" />{label}</button>)}
        </div>
        <span className="flex-1" />
        <Button size="icon-xs" variant="ghost" aria-label="刷新项目" title="刷新项目" disabled={busy} onClick={() => void refresh()}><RefreshCw /></Button>
        <Button size="icon-xs" variant={creating ? "secondary" : "ghost"} aria-label="新建项目" title="新建项目" aria-pressed={creating} disabled={busy} onClick={() => setCreating(!creating)}><FolderPlus /></Button>
        </div>
        <div id="project-sidebar-panel-projects" role="tabpanel" aria-labelledby="project-sidebar-tab-projects" hidden={sidebarTab !== "projects"} className="min-h-0 flex-1 overflow-auto py-2">
          {projects.length === 0 && <p className="px-3 py-2 text-xs text-muted-foreground">暂无项目</p>}
          {projects.map((item) => <button type="button" key={item.name} disabled={busy} onClick={() => void selectProject(item.name)} className={`w-full px-3 py-2 text-left text-sm hover:bg-accent ${project === item.name ? "bg-accent font-medium" : ""}`}><span className="block truncate">{item.name}</span><span className="block truncate text-xs text-muted-foreground">{item.error ? "需要修复" : item.status}</span></button>)}
        </div>
        <div id="project-sidebar-panel-files" role="tabpanel" aria-labelledby="project-sidebar-tab-files" hidden={sidebarTab !== "files"} className="min-h-0 flex-1 overflow-auto py-2">
          {!project ? <p className="px-3 py-2 text-xs text-muted-foreground">请先选择项目</p> : <>
            <div className="flex items-center gap-1 px-2 py-1">
              <div className="min-w-0 flex-1 truncate text-xs font-semibold text-muted-foreground" title={project}>{project}</div>
              <Button size="icon-xs" variant="ghost" aria-label="重建开发容器" title="重建开发容器（使用已保存的文件）" disabled={busy || !connected || !!debugRef.current} onClick={() => void selectProject(project, true)}><RefreshCw /></Button>
              <Button size="icon-xs" variant="ghost" aria-label="新建文件" title="新建文件" disabled={busy || !connected} onClick={() => void createFile()}><FilePlus2 /></Button>
              <Button size="icon-xs" variant="ghost" aria-label="新建文件夹" title="新建文件夹" disabled={busy || !connected} onClick={() => void createDirectory()}><FolderPlus /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={treeSelection?.kind === "directory" ? "重命名文件夹" : "重命名文件"} title="重命名所选项目" disabled={busy || !connected || !treeSelection || !!debugRef.current} onClick={() => void renameFile()}><Pencil /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={treeSelection?.kind === "directory" ? "删除文件夹" : "删除文件"} title="删除所选项目" disabled={busy || !connected || !treeSelection || !!debugRef.current} onClick={() => void deleteFile()}><Trash2 /></Button>
            </div>
            {!connected ? <p className="px-3 py-2 text-xs text-muted-foreground">开发容器未连接</p> : loadingFiles && files.length === 0 && directories.length === 0 ? <p className="px-3 py-2 text-xs text-muted-foreground">文件加载中…</p> : files.length === 0 && directories.length === 0 ? <p className="px-3 py-2 text-xs text-muted-foreground">暂无文件</p> : null}
            <ProjectFileTree key={project} files={files} directories={directories} selected={treeSelection?.path ?? ""} disabled={busy || !connected} directoryMove={directoryMove} onOpen={(file) => void openFile(file, undefined, true)} onSelectDirectory={(folder) => setTreeSelection({ path: folder, kind: "directory" })} />
          </>}
        </div>
      </aside>, slot)}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {selected?.error && <div role="status" className="border-b bg-amber-500/10 p-3 text-xs">项目 manifest 需要修复：{selected.error}</div>}
        {vscodeUrl && <div className="flex shrink-0 gap-1 border-b px-2 py-1"><Button size="xs" variant={showVscode ? "secondary" : "ghost"} onClick={() => setShowVscode(true)}>VS Code 工作台</Button>{path && <Button size="xs" variant={!showVscode ? "secondary" : "ghost"} onClick={() => setShowVscode(false)}>项目编辑器</Button>}</div>}
        {Object.entries(vscodeFrames).map(([frameProject, url]) => <iframe key={frameProject} title={`${frameProject} VS Code 工作台`} src={url} className={`min-h-0 w-full flex-1 border-0 ${showVscode && project === frameProject ? "" : "hidden"}`} inert={project === frameProject && !connected} sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" allow="clipboard-read; clipboard-write" referrerPolicy="no-referrer" />)}
        <div className={`flex min-h-0 min-w-0 flex-1 flex-col ${showVscode && vscodeUrl ? "hidden" : ""}`}>
        {path && conflicts[path] && <div role="status" className="flex items-center gap-2 border-b bg-amber-500/10 px-3 py-2 text-xs"><span>{conflicts[path] === "deleted" ? "该文件已在容器中删除，当前缓冲区仍被保留。" : "该文件已在容器中更改，当前未保存内容仍被保留。"}</span>{conflicts[path] === "changed" && <Button size="xs" variant="outline" disabled={busy || !connected} onClick={() => void reloadFile()}>重新读取</Button>}</div>}
        {Object.keys(buffers).length > 0 && <div className="flex shrink-0 overflow-x-auto border-b" role="tablist" aria-label="打开的项目文件">{Object.entries(buffers).map(([file, buffer]) => <div key={file} className={`flex shrink-0 items-center border-r ${file === path ? "bg-accent" : "text-muted-foreground"}`}><button type="button" role="tab" aria-selected={file === path} title={file} disabled={busy} className="max-w-48 truncate px-3 py-2 text-xs" onClick={() => void openFile(file)}>{file.split("/").at(-1)}{buffer.contents !== buffer.original ? " •" : ""}</button><button type="button" aria-label={`关闭 ${file}`} title={`关闭 ${file}`} disabled={busy} className="mr-1 rounded p-1 hover:bg-muted" onClick={() => closeBuffer(file)}><X className="size-3" /></button></div>)}</div>}
        {path ? <><div className="flex items-center gap-2 border-b px-4 py-2 text-sm"><span className="min-w-0 flex-1 truncate" title={path}>{path}{dirty ? " •" : ""}</span>{path.endsWith(".py") && <Button size="sm" variant="outline" disabled={dirty || busy || !connected || !!debugRef.current} title={dirty ? "请先保存文件" : "在开发容器中调试"} onClick={() => void startDebug()}>开始调试</Button>}<Button size="sm" variant="outline" disabled={!dirty || busy || !connected} onClick={() => void save()}><Save /> 保存</Button></div><CodeEditor project={project} path={path} value={contents} readOnly={!connected || busy} lspEnabled={connected} saveVersion={saveVersion} navigation={navigation} breakpoints={breakpoints[path]} onToggleBreakpoint={toggleBreakpoint} onNavigate={(file, line, column) => openFile(file, { line, column })} onChange={changeContents} onSave={() => void save()} onApplyEdits={applyWorkspaceEdits} /></> : <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-muted-foreground">{project ? "从左侧选择文件开始编辑" : "选择或创建一个插件项目"}</div>}
        </div>
        {debugStatus && <div className="shrink-0 border-t px-3 py-2 text-xs"><div className="flex flex-wrap items-center gap-2"><span role="status">{debugStatus}</span>{debugRef.current && <><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("continue")}>继续</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("next")}>单步跳过</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("stepIn")}>单步进入</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("stepOut")}>单步跳出</Button><Button size="xs" variant="outline" onClick={stopDebug}>停止</Button></>}</div>{debugThread !== undefined && debugRef.current && <DebugInspector key={debugStop} client={debugRef.current} thread={debugThread} onNavigate={(file, line, column) => void openFile(file, { line, column })} />}{debugOutput && <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap">{debugOutput}</pre>}</div>}
        <Button className="m-2 self-start" size="sm" variant="ghost" onClick={() => { setPanelTab("terminal"); setPanelVisible(true); }}><TerminalSquare />终端</Button>
      </div>
  </section>;
}
