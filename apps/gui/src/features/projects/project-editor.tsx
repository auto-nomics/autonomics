import { boundedContextText, useEditorContextSnapshot } from "@/app/workbench/editor-context-snapshots";
import { ViewLocationMenu } from "@/app/workbench/view-location-menu";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useProjectSidebarSlot } from "./sidebar-slot";
import { PluginOverview, type InstalledPlugin, type PluginProject } from "./plugin-manager";
import { PluginTools } from "./plugin-tools";
import { ProjectFileTree } from "./project-file-tree";
import { CodeEditor } from "./code-editor";
import { DebugClient } from "./debug-client";
import { DebugInspector } from "./debug-inspector";
import { applyTextEdits, type FileEdits } from "./workspace-edit";
import { publishProjectChanges, type ProjectFileChange } from "./watch-events";
import { FileCode2, FilePlus2, Folder, FolderPlus, PanelLeftOpen, Pencil, RefreshCw, Save, StopCircle, TerminalSquare, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getGroup, patchEditorState, setContainer, setPanelTab, setPanelVisible, setSidebarCollapsed, useWorkbenchState, type EditorRef } from "@/app/workbench/workbench-store";

type Environment = { id: string; reference: string };
type Buffer = { contents: string; original: string };

export function ProjectEditor({ editor }: { editor: EditorRef }) {
  const { t } = useTranslation("projects");
  const slot = useProjectSidebarSlot();
  const workbench = useWorkbenchState();
  const group = getGroup(workbench.groups, workbench.groups.active);
  const active = group?.activeId === editor.id;
  const [sidebarTab, setSidebarTab] = useState<"projects" | "files">("projects");
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [filesRevision, setFilesRevision] = useState(0);
  const [saveVersion, setSaveVersion] = useState(0);
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [projects, setProjects] = useState<PluginProject[]>([]);
  const [installed, setInstalled] = useState<InstalledPlugin[]>([]);
  const [overview, setOverview] = useState(true);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [project, setProject] = useState("");
  const [recovery, setRecovery] = useState(false);
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
  useEditorContextSnapshot(editor.id, { project, path, dirty: localDirty, ...boundedContextText(contents), scope: t("project.contextScope") });
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
    setDebugOutput(""); setDebugStatus(t("debug.starting")); setDebugThread(undefined);
    const client = new DebugClient(project, message => {
      if (message.event === "output") setDebugOutput(current => (current + String(message.body?.output ?? "")).slice(-30000));
      if (message.event === "stopped") {
        const threadId = message.body?.threadId;
        setDebugThread(threadId); setDebugStop(value => value + 1); setDebugStatus(t("debug.paused", { reason: message.body?.reason ?? t("debug.breakpoint") }));
      }
      if (message.event === "continued") { setDebugThread(undefined); setDebugStatus(t("debug.running")); }
      if (message.event === "terminated") { setDebugThread(undefined); setDebugStatus(t("debug.terminated")); client.dispose(); if (debugRef.current === client) debugRef.current = null; }
    }, cause => { setDebugStatus(t("debug.failed", { error: cause.message })); if (debugRef.current === client) debugRef.current = null; });
    debugRef.current = client;
    try { await client.start(path, breakpoints); if (debugRef.current === client) setDebugStatus(current => current.startsWith(t("debug.pausedPrefix")) ? current : t("debug.running")); }
    catch (cause) { if (debugRef.current === client) { setDebugStatus(t("debug.failed", { error: String(cause) })); debugRef.current = null; client.dispose(); } }
  };
  const debugCommand = (command: string) => { if (debugRef.current && debugThread !== undefined) void debugRef.current.request(command, { threadId: debugThread }).catch(cause => setDebugStatus(String(cause))); };
  const stopDebug = () => { const client = debugRef.current; debugRef.current = null; setDebugThread(undefined); setDebugStatus(t("debug.stopped")); if (client) void client.request("disconnect", { terminateDebuggee: true }).catch(() => {}).finally(() => client.dispose()); };

  const refresh = useCallback(async () => {
    try {
      const [list, env, plugins] = await Promise.all([gatewayRequest("GET", "/plugin-projects"), gatewayRequest("GET", "/plugins/environments"), gatewayRequest("GET", "/plugins")]);
      setProjects(list.projects ?? []); setEnvironments(env.environments ?? []); setInstalled(plugins.plugins ?? []); setProjectsLoaded(true);
      setEnvironment((current) => current || env.environments?.[0]?.id || "");
      setFilesRevision(value => value + 1);
      setError("");
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    }
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
    if (!project || recovery) return;
    let cancelled = false;
    const check = async () => {
      try {
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/container`);
        if (!cancelled) setConnected(result.connected === true);
      } catch { if (!cancelled) setConnected(false); }
    };
    const timer = window.setInterval(() => void check(), 5000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [project, recovery]);
  useEffect(() => {
    if (recovery) return;
    if (!project || !connected) { setFiles([]); setDirectories([]); setLoadingFiles(false); return; }
    let cancelled = false;
    setLoadingFiles(true);
    gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/files`)
      .then((result) => { if (!cancelled) { setFiles(result.files); setDirectories(result.directories ?? []); } })
      .catch((cause) => { if (!cancelled) setError(String(cause)); })
      .finally(() => { if (!cancelled) setLoadingFiles(false); });
    return () => { cancelled = true; };
  }, [project, connected, filesRevision, recovery]);
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
        if (!Array.isArray(result.entries)) throw new Error(t("project.snapshotInvalid"));
        const next = new Map<string, Entry>();
        for (const entry of result.entries as Entry[]) {
          if (!entry || typeof entry.path !== "string" || !["file", "directory"].includes(entry.kind) || typeof entry.stamp !== "string") throw new Error(t("project.snapshotInvalid"));
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
            if (typeof fileResult.contents !== "string") throw new Error(t("project.fileContentInvalid"));
            pending.delete(file);
            if (fileResult.contents === buffer.original) { clearConflict(file); continue; }
            if (buffer.contents !== buffer.original) { setConflicts(current => ({ ...current, [file]: "changed" })); continue; }
            const updated = { contents: fileResult.contents, original: fileResult.contents };
            setBuffers(current => current[file] === buffer ? { ...current, [file]: updated } : current);
            if (latest.path === file) { setContents(updated.contents); setOriginal(updated.original); }
            clearConflict(file);
          } catch { /* Retry on the next snapshot; the file may be mid-write. */ }
        }
      } catch (cause) { if (!cancelled) setWatchError(t("project.watchUnavailable", { error: String(cause) })); }
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

  const mayLeave = () => !(localDirty || vscodeDirty[project]) || window.confirm(t("project.confirmDiscard"));
  const selectProject = async (next: string, rebuild = false) => {
    if (next !== project && !mayLeave()) return;
    if (rebuild && (localDirty || vscodeDirty[project]) && !window.confirm(t("project.confirmRebuild"))) return;
    setBusy(true); setConnecting(next); setError("");
    if (rebuild) setConnected(false);
    try {
      const needsRepair = !!projects.find(item => item.name === next)?.error;
      if (needsRepair) {
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(next)}/dev-files`);
        setFiles(result.files ?? []); setDirectories([]);
      } else {
        await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(next)}/container${rebuild ? "?rebuild=true" : ""}`);
      }
      setRecovery(needsRepair);
      if (rebuild) setContainerVersion(value => value + 1);
      setConnected(!needsRepair);
      setFilesRevision(value => value + 1);
      setSidebarTab("files"); setOverview(false);
      if (next !== project) { setProject(next); setBuffers({}); setConflicts({}); setBreakpoints({}); setDebugStatus(""); setDebugOutput(""); setDebugThread(undefined); setPath(""); setTreeSelection(undefined); setDirectoryMove(undefined); setNavigation(undefined); setContents(""); setOriginal(""); if (!needsRepair) { setFiles([]); setDirectories([]); } }
    } catch (cause) {
      if (next === project) {
        try { const status = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(next)}/container`); setConnected(status.connected === true); }
        catch { setConnected(false); }
      }
      setError(t("project.connectFailed", { error: String(cause) }));
    }
    finally { setBusy(false); setConnecting(""); }
  };
  const initialProject = useRef(String(editor.params.name ?? (editor.state as { project?: string } | null)?.project ?? ""));
  useEffect(() => {
    if (projectsLoaded && initialProject.current) { const name = initialProject.current; initialProject.current = ""; void selectProject(name); }
  }, [projectsLoaded]);

  const pluginAction = async (name: string, action: "uninstall" | "rollback") => {
    if (!window.confirm(action === "uninstall" ? t("project.confirmUninstall", { name }) : t("project.confirmRollback", { name }))) return;
    setBusy(true); setError("");
    try { await gatewayRequest("POST", `/plugins/${encodeURIComponent(name)}/${action}`); await refresh(); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const stopContainer = async () => {
    if ((localDirty || vscodeDirty[project]) && !window.confirm(t("project.confirmStop"))) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("DELETE", `/plugin-projects/${encodeURIComponent(project)}/container`);
      debugRef.current?.dispose(); debugRef.current = null;
      setConnected(false); setShowVscode(false);
      setVscodeFrames(current => { const next = { ...current }; delete next[project]; return next; });
      setVscodeDirty(current => { const next = { ...current }; delete next[project]; return next; });
      setContainerVersion(value => value + 1);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const openFile = async (next: string, target?: { line: number; column: number }, preferVscode = false): Promise<boolean> => {
    if (!connected && !recovery) return false;
    const cachedDirty = buffers[next] && buffers[next].contents !== buffers[next].original;
    if (preferVscode && vscodeUrl && !cachedDirty) {
      setBusy(true); setError("");
      try {
        const result = await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(project)}/vscode-control`, { action: "open", path: next, ...target });
        if (result?.opened === true) { setTreeSelection({ path: next, kind: "file" }); setShowVscode(true); return true; }
      } catch (cause) { setError(t("project.vscodeOpenFailed", { error: String(cause) })); return false; }
      finally { setBusy(false); }
    }
    autoShowVscode.current = false;
    setShowVscode(false);
    if (next === path) { setTreeSelection({ path: next, kind: "file" }); if (target) setNavigation(current => ({ path: next, ...target, id: (current?.id ?? 0) + 1 })); return true; }
    const cached = buffers[next];
    if (cached) { setPath(next); setContents(cached.contents); setOriginal(cached.original); setTreeSelection({ path: next, kind: "file" }); setNavigation(current => target ? { path: next, ...target, id: (current?.id ?? 0) + 1 } : undefined); return true; }
    setBusy(true); setError("");
    try {
      const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(project)}/${recovery ? "dev-file" : "file"}?path=${encodeURIComponent(next)}`);
      setBuffers(current => ({ ...current, [next]: { contents: result.contents, original: result.contents } }));
      setPath(next); setContents(result.contents); setOriginal(result.contents);
      setTreeSelection({ path: next, kind: "file" });
      setNavigation(current => target ? { path: next, ...target, id: (current?.id ?? 0) + 1 } : undefined);
      return true;
    } catch (cause) { setError(String(cause)); return false; }
    finally { setBusy(false); }
  };
  const save = async () => {
    if ((!connected && !recovery) || !path || !dirty || busy) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest("PUT", `/plugin-projects/${encodeURIComponent(project)}/${recovery ? "dev-file" : "file"}`, { path, contents, original });
      setOriginal(contents);
      setBuffers(current => ({ ...current, [path]: { contents, original: contents } }));
      setConflicts(current => { const next = { ...current }; delete next[path]; return next; });
      setSaveVersion(version => version + 1);
      if (recovery) await refresh();
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const reloadFile = async () => {
    if (!connected || busy || !path || (dirty && !window.confirm(t("project.confirmReload")))) return;
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
    if (!snapshot.connected || snapshot.busy || workspaceEditLock.current || !valid()) throw new Error(t("project.notEditable"));
    if (edits.some(edit => edit.path !== snapshot.path && snapshot.buffers[edit.path] && snapshot.buffers[edit.path].contents !== snapshot.buffers[edit.path].original)) throw new Error(t("project.saveOthersFirst"));
    const revision = editRevision.current;
    workspaceEditLock.current = true; setBusy(true);
    try {
      const loaded = await Promise.all(edits.map(async edit => {
        const buffer = snapshot.buffers[edit.path];
        if (buffer) return buffer;
        const result = await gatewayRequest("GET", `/plugin-projects/${encodeURIComponent(snapshot.project)}/file?path=${encodeURIComponent(edit.path)}`);
        if (typeof result.contents !== "string") throw new Error(t("project.readFailed", { path: edit.path }));
        return { contents: result.contents, original: result.contents };
      }));
      if (!valid() || revision !== editRevision.current || workspaceState.current.project !== snapshot.project || workspaceState.current.path !== snapshot.path || workspaceState.current.buffers !== snapshot.buffers || !workspaceState.current.connected) throw new Error(t("project.retryChanged"));
      const next = { ...snapshot.buffers };
      edits.forEach((edit, index) => { next[edit.path] = { contents: applyTextEdits(loaded[index].contents, edit.edits), original: loaded[index].original }; });
      setBuffers(next);
      if (next[snapshot.path]) setContents(next[snapshot.path].contents);
      return edits.some(edit => edit.path === snapshot.path) ? next[snapshot.path]?.contents : undefined;
    } finally { workspaceEditLock.current = false; setBusy(false); }
  };
  const closeBuffer = (file: string) => {
    const buffer = buffers[file];
    if (!buffer || (buffer.contents !== buffer.original && !window.confirm(t("project.confirmCloseFile", { file })))) return;
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
    const next = window.prompt(t("project.newFilePathPrompt"), treeSelection?.kind === "directory" ? `${treeSelection.path}/` : "");
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
    const next = window.prompt(t("project.newDirectoryPathPrompt"), treeSelection?.kind === "directory" ? `${treeSelection.path}/` : "");
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
    const next = window.prompt(t("project.renamePathPrompt", { kind: isDirectory ? t("project.renameDirectoryKind") : t("project.renameFileKind") }), previous);
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
    if (!window.confirm(t("project.confirmDelete", { kind: isDirectory ? t("project.deleteDirectoryKind") : t("project.deleteFileKind"), path: selectedPath, note: losesChanges ? t("project.deleteLosesChanges") : "" }))) return;
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
    event.preventDefault(); if (!mayLeave()) return;
    setBusy(true);
    setError("");
    try {
      await gatewayRequest("POST", "/plugin-projects", { name, environment_id: environment, purpose });
      await refresh(); setCreating(false); setName(""); setPurpose("");
      await gatewayRequest("POST", `/plugin-projects/${encodeURIComponent(name)}/container`);
      setConnected(true); setRecovery(false);
      setSidebarTab("files"); setOverview(false); setProject(name); setBuffers({}); setConflicts({}); setBreakpoints({}); setDebugStatus(""); setDebugOutput(""); setDebugThread(undefined); setPath(""); setTreeSelection(undefined); setDirectoryMove(undefined); setNavigation(undefined); setContents(""); setOriginal(""); setFiles([]); setDirectories([]);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  const selected = projects.find((item) => item.name === project);
  return <section aria-label={t("project.title")} className="flex min-h-0 min-w-0 flex-1 flex-col" onKeyDown={(event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); }
  }}>
    <header className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2">
      <h1 className="mr-auto text-sm font-semibold">{t("project.title")}{!overview && project ? ` · ${project}` : ""}</h1>
      <Button size="sm" variant={overview ? "secondary" : "ghost"} onClick={() => setOverview(true)}>{t("project.overview")}</Button>
      {project && <Button size="sm" variant={!overview ? "secondary" : "ghost"} onClick={() => setOverview(false)}>{t("project.backToProject")}</Button>}
      <Button size="sm" variant="outline" disabled={busy} onClick={() => void refresh()}><RefreshCw />{t("project.refresh")}</Button>
      <Button size="sm" disabled={busy} onClick={() => setCreating(!creating)}>{t("project.newPlugin")}</Button>
    </header>
    {(workbench.sidebar.collapsed || workbench.sidebar.container !== "projects") && <div className="flex shrink-0 border-b px-2 py-1">
      <Button size="sm" variant="ghost" onClick={() => { setContainer("projects"); setSidebarCollapsed(false); }}><PanelLeftOpen />{t("project.pluginsAndFiles")}</Button>
    </div>}
    {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
    {watchError && <div role="status" className="border-b px-4 py-2 text-xs text-muted-foreground">{watchError}</div>}
    {connecting && <div role="status" className="border-b px-4 py-2 text-xs text-muted-foreground">{t("project.connectingContainer", { name: connecting })}</div>}
    {project && !connected && <div role="status" className="flex items-center gap-2 border-b bg-amber-500/10 px-4 py-2 text-xs">{recovery ? t("project.recoveryBanner") : t("project.disconnectedBanner")}<Button size="xs" variant="outline" disabled={busy} onClick={() => void selectProject(project)}>{t("project.reconnect")}</Button></div>}
    {creating && <form onSubmit={(event) => void create(event)} className="grid gap-2 border-b bg-muted/30 p-4 sm:grid-cols-2">
      <label className="grid gap-1 text-xs">{t("project.nameLabel")}<Input required pattern="[a-z][a-z0-9-]*[a-z0-9]|[a-z]" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-plugin" /></label>
      <label className="grid gap-1 text-xs">{t("project.environmentLabel")}<select required className="h-9 rounded-md border bg-background px-2 text-sm" value={environment} onChange={(event) => setEnvironment(event.target.value)}><option value="">{t("project.environmentPlaceholder")}</option>{environments.map((item) => <option key={item.id} value={item.id}>{item.id}</option>)}</select></label>
      <label className="grid gap-1 text-xs sm:col-span-2">{t("project.purposeLabel")}<Input required value={purpose} onChange={(event) => setPurpose(event.target.value)} /></label>
      <div className="flex gap-2 sm:col-span-2"><Button disabled={busy || !environment} size="sm" type="submit">{t("project.createAndOpen")}</Button><Button size="sm" type="button" variant="ghost" onClick={() => setCreating(false)}>{t("project.cancel")}</Button></div>
    </form>}
    {active && slot && createPortal(
      <aside aria-label={t("project.navAriaLabel")} className="flex min-h-0 flex-1 flex-col">
        <div className="flex h-9 shrink-0 items-center gap-0.5 border-b px-1">
        <div role="tablist" aria-label={t("project.sidebarAriaLabel")} className="flex min-w-0 items-center gap-0.5">
          {([
            { id: "projects", label: t("project.tabProjects"), icon: Folder },
            { id: "files", label: t("project.tabFiles"), icon: FileCode2 },
          ] as const).map(({ id, label, icon: Icon }) => <button
            key={id} id={`project-sidebar-tab-${id}`} type="button" role="tab"
            aria-selected={sidebarTab === id} aria-controls={`project-sidebar-panel-${id}`}
            tabIndex={sidebarTab === id ? 0 : -1}
            className={`flex h-7 items-center gap-1 rounded-md px-1 text-xs ${sidebarTab === id ? "bg-secondary text-secondary-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"}`}
            onClick={() => { setSidebarTab(id); if (id === "files" && project) setOverview(false); }}
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
        <Button size="icon-xs" variant="ghost" aria-label={t("project.refreshProject")} title={t("project.refreshProject")} disabled={busy} onClick={() => void refresh()}><RefreshCw /></Button>
        <Button size="icon-xs" variant={creating ? "secondary" : "ghost"} aria-label={t("project.newPlugin")} title={t("project.newPlugin")} aria-pressed={creating} disabled={busy} onClick={() => setCreating(!creating)}><FolderPlus /></Button>
        <ViewLocationMenu id="projects" primaryOnly />
        </div>
        <div id="project-sidebar-panel-projects" role="tabpanel" aria-labelledby="project-sidebar-tab-projects" hidden={sidebarTab !== "projects"} className="min-h-0 flex-1 overflow-auto py-2">
          {projects.length === 0 && <p className="px-3 py-2 text-xs text-muted-foreground">{t("project.noProjects")}</p>}
          {projects.map((item) => <button type="button" key={item.name} disabled={busy} onClick={() => void selectProject(item.name)} className={`w-full px-3 py-2 text-left text-sm hover:bg-accent ${project === item.name ? "bg-accent font-medium" : ""}`}><span className="block truncate">{item.name}</span><span className="block truncate text-xs text-muted-foreground">{item.error ? t("project.needsRepair") : item.status}</span></button>)}
        </div>
        <div id="project-sidebar-panel-files" role="tabpanel" aria-labelledby="project-sidebar-tab-files" hidden={sidebarTab !== "files"} className="min-h-0 flex-1 overflow-auto py-2">
          {!project ? <p className="px-3 py-2 text-xs text-muted-foreground">{t("project.selectProjectFirst")}</p> : <>
            <div className="flex items-center gap-1 px-2 py-1">
              <div className="min-w-0 flex-1 truncate text-xs font-semibold text-muted-foreground" title={project}>{project}</div>
              <Button size="icon-xs" variant="ghost" aria-label={t("project.rebuildContainer")} title={t("project.rebuildContainerTitle")} disabled={busy || !connected || !!debugRef.current} onClick={() => void selectProject(project, true)}><RefreshCw /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={t("project.newFile")} title={t("project.newFile")} disabled={busy || !connected} onClick={() => void createFile()}><FilePlus2 /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={t("project.newDirectory")} title={t("project.newDirectory")} disabled={busy || !connected} onClick={() => void createDirectory()}><FolderPlus /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={treeSelection?.kind === "directory" ? t("project.renameDirectory") : t("project.renameFile")} title={t("project.renameSelected")} disabled={busy || !connected || !treeSelection || !!debugRef.current} onClick={() => void renameFile()}><Pencil /></Button>
              <Button size="icon-xs" variant="ghost" aria-label={treeSelection?.kind === "directory" ? t("project.deleteDirectory") : t("project.deleteFile")} title={t("project.deleteSelected")} disabled={busy || !connected || !treeSelection || !!debugRef.current} onClick={() => void deleteFile()}><Trash2 /></Button>
            </div>
            {!connected && !recovery ? <p className="px-3 py-2 text-xs text-muted-foreground">{t("project.containerNotConnected")}</p> : loadingFiles && files.length === 0 && directories.length === 0 ? <p className="px-3 py-2 text-xs text-muted-foreground">{t("project.loadingFiles")}</p> : files.length === 0 && directories.length === 0 ? <p className="px-3 py-2 text-xs text-muted-foreground">{t("project.noFiles")}</p> : null}
            <ProjectFileTree key={project} files={files} directories={directories} selected={treeSelection?.path ?? ""} disabled={busy || (!connected && !recovery)} directoryMove={directoryMove} onOpen={(file) => void openFile(file, undefined, true)} onSelectDirectory={(folder) => setTreeSelection({ path: folder, kind: "directory" })} />
          </>}
        </div>
      </aside>, slot)}
      {overview && <PluginOverview projects={projects} installed={installed} busy={busy} onOpen={name => void selectProject(name)} onAction={(name, action) => void pluginAction(name, action)} />}
      <div className={`flex min-h-0 min-w-0 flex-1 flex-col ${overview ? "hidden" : ""}`}>
        {project && <div className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-1">
          <span className="mr-auto text-xs text-muted-foreground" data-dev-session-state={connected ? "running" : "exited"}>{t("project.containerLabel")}{connected ? t("project.containerConnected") : t("project.containerDisconnected")}</span>
          <Button size="xs" variant="ghost" disabled={busy || !connected || !!debugRef.current} onClick={() => void selectProject(project, true)}><RefreshCw />{t("project.restartContainer")}</Button>
          <Button size="xs" variant="ghost" disabled={busy || !connected} onClick={() => void stopContainer()}><StopCircle />{t("project.stopContainer")}</Button>
        </div>}
        {selected?.error && <div role="status" className="border-b bg-amber-500/10 p-3 text-xs">{t("project.manifestNeedsRepair", { error: selected.error })}</div>}
        {vscodeUrl && <div className="flex shrink-0 gap-1 border-b px-2 py-1"><Button size="xs" variant={showVscode ? "secondary" : "ghost"} onClick={() => setShowVscode(true)}>{t("project.vscodeWorkbench")}</Button>{path && <Button size="xs" variant={!showVscode ? "secondary" : "ghost"} onClick={() => setShowVscode(false)}>{t("project.projectEditor")}</Button>}</div>}
        {Object.entries(vscodeFrames).map(([frameProject, url]) => <iframe key={frameProject} title={t("project.vscodeFrameTitle", { project: frameProject })} src={url} className={`min-h-0 w-full flex-1 border-0 ${showVscode && project === frameProject ? "" : "hidden"}`} inert={project === frameProject && !connected} sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" allow="clipboard-read; clipboard-write" referrerPolicy="no-referrer" />)}
        <div className={`flex min-h-0 min-w-0 flex-1 flex-col ${showVscode && vscodeUrl ? "hidden" : ""}`}>
        {path && conflicts[path] && <div role="status" className="flex items-center gap-2 border-b bg-amber-500/10 px-3 py-2 text-xs"><span>{conflicts[path] === "deleted" ? t("project.conflictDeleted") : t("project.conflictChanged")}</span>{conflicts[path] === "changed" && <Button size="xs" variant="outline" disabled={busy || !connected} onClick={() => void reloadFile()}>{t("project.reload")}</Button>}</div>}
        {Object.keys(buffers).length > 0 && <div className="flex shrink-0 overflow-x-auto border-b" role="tablist" aria-label={t("project.openFilesAria")}>{Object.entries(buffers).map(([file, buffer]) => <div key={file} className={`flex shrink-0 items-center border-r ${file === path ? "bg-accent" : "text-muted-foreground"}`}><button type="button" role="tab" aria-selected={file === path} title={file} disabled={busy} className="max-w-48 truncate px-3 py-2 text-xs" onClick={() => void openFile(file)}>{file.split("/").at(-1)}{buffer.contents !== buffer.original ? " •" : ""}</button><button type="button" aria-label={t("project.closeFile", { file })} title={t("project.closeFile", { file })} disabled={busy} className="mr-1 rounded p-1 hover:bg-muted" onClick={() => closeBuffer(file)}><X className="size-3" /></button></div>)}</div>}
        {path ? <><div className="flex items-center gap-2 border-b px-4 py-2 text-sm"><span className="min-w-0 flex-1 truncate" title={path}>{path}{dirty ? " •" : ""}</span>{path.endsWith(".py") && <Button size="sm" variant="outline" disabled={dirty || busy || !connected || !!debugRef.current} title={dirty ? t("project.saveFirst") : t("project.debugTitle")} onClick={() => void startDebug()}>{t("project.startDebug")}</Button>}<Button size="sm" variant="outline" disabled={!dirty || busy || (!connected && !recovery)} onClick={() => void save()}><Save /> {t("project.save")}</Button></div><CodeEditor project={project} path={path} value={contents} readOnly={(!connected && !recovery) || busy} lspEnabled={connected} saveVersion={saveVersion} navigation={navigation} breakpoints={breakpoints[path]} onToggleBreakpoint={toggleBreakpoint} onNavigate={(file, line, column) => openFile(file, { line, column })} onChange={changeContents} onSave={() => void save()} onApplyEdits={applyWorkspaceEdits} /></> : <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-muted-foreground">{project ? t("project.pickFileHint") : t("project.pickProjectHint")}</div>}
        </div>
        {debugStatus && <div className="shrink-0 border-t px-3 py-2 text-xs"><div className="flex flex-wrap items-center gap-2"><span role="status">{debugStatus}</span>{debugRef.current && <><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("continue")}>{t("debug.continue")}</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("next")}>{t("debug.stepOver")}</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("stepIn")}>{t("debug.stepInto")}</Button><Button size="xs" variant="outline" disabled={debugThread === undefined} onClick={() => debugCommand("stepOut")}>{t("debug.stepOut")}</Button><Button size="xs" variant="outline" onClick={stopDebug}>{t("debug.stop")}</Button></>}</div>{debugThread !== undefined && debugRef.current && <DebugInspector key={debugStop} client={debugRef.current} thread={debugThread} onNavigate={(file, line, column) => void openFile(file, { line, column })} />}{debugOutput && <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap">{debugOutput}</pre>}</div>}
        {project && <PluginTools key={project} project={project} dirty={localDirty || !!vscodeDirty[project]} disabled={busy || (!connected && !recovery)} saveVersion={saveVersion + filesRevision} onChanged={refresh} />}
        <Button className="m-2 self-start" size="sm" variant="ghost" onClick={() => { setPanelTab("terminal"); setPanelVisible(true); }}><TerminalSquare />{t("project.terminal")}</Button>
      </div>
  </section>;
}
