/**
 * VFS browser sidebar: a lazy directory tree over the whole virtual
 * namespace plus file management (new folder / new file / upload /
 * rename / delete / download). Mutations go through the /api/v1/vfs
 * write endpoints; read-only mounts are enforced server-side and the
 * toolbar/buttons disable on them as a UX affordance. Destructive
 * operations confirm client-side on top of the server contract.
 */

import { useEffect, useState } from "react";
import { useSyncExternalStore } from "react";
import {
  ChevronDown, ChevronRight, Download, FilePlus2, Folder, FolderOpen, FolderPlus, Lock,
  Pencil, RefreshCw, Server, Trash2, Upload,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  bootstrapTree, getVfsTreeState, reloadSubtree,
  resetTree, subscribeVfsTree, toggleDir, readOnlyCover, type TreeNode,
} from "./vfs-tree";
import {
  downloadVfsFile, formatBytes, mkdir, moveEntry, pickUploadSource,
  removeEntry, saveTextFile, uploadToDir,
} from "./vfs-api";
import { openOrDownloadVfsFile } from "./vfs-file";

function parentOf(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  return index <= 0 ? "/" : trimmed.slice(0, index);
}

function Row({
  node,
  depth,
  active,
  onSelect,
  onMutated,
}: {
  node: TreeNode;
  depth: number;
  active: boolean;
  onSelect: (node: TreeNode) => void;
  onMutated: (parent: string) => void;
}) {
  const state = getVfsTreeState();
  const isOpen = state.expanded.has(node.path);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const rename = async () => {
    const next = window.prompt("重命名为:", node.name);
    if (!next || next === node.name) return;
    setBusy(true);
    setError("");
    try {
      await moveEntry(node.path, `${parentOf(node.path) === "/" ? "" : parentOf(node.path)}/${next}`);
      onMutated(parentOf(node.path));
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    const message = node.is_dir
      ? `删除目录 ${node.path} 及其全部内容?`
      : `删除文件 ${node.path}?`;
    if (!window.confirm(message)) return;
    setBusy(true);
    setError("");
    try {
      await removeEntry(node.path, node.is_dir);
      onMutated(parentOf(node.path));
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const download = async () => {
    try {
      await downloadVfsFile(node.path);
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    }
  };

  return (
    <div className="group relative">
      <button
        type="button"
        role={node.is_dir ? "treeitem" : undefined}
        aria-expanded={node.is_dir ? isOpen : undefined}
        aria-current={active ? "true" : undefined}
        aria-label={node.path}
        title={`${node.path}${node.is_dir ? "" : ` · ${formatBytes(node.size)}`}`}
        data-vfs-path={node.path}
        data-vfs-dir={node.is_dir || undefined}
        disabled={busy}
        onClick={() => (node.is_dir ? (toggleDir(node.path), onSelect(node)) : void openOrDownloadVfsFile(node.path, node.readOnly))}
        className={cn(
          "flex w-full items-center gap-1 rounded-sm px-1 py-0.5 text-left text-xs hover:bg-accent",
          !node.is_dir && "cursor-pointer",
          active && node.is_dir && "bg-accent",
        )}
        style={{ paddingLeft: `${depth * 12 + 4}px` }}
      >
        {node.is_dir ? (
          <>
            {isOpen ? <ChevronDown className="size-3 shrink-0 text-muted-foreground" /> : <ChevronRight className="size-3 shrink-0 text-muted-foreground" />}
            {isOpen ? <FolderOpen className="size-3.5 shrink-0 text-primary" /> : <Folder className="size-3.5 shrink-0 text-primary" />}
          </>
        ) : (
          <span className="size-3 shrink-0" aria-hidden />
        )}
        {node.readOnly && <Lock className="size-3 shrink-0 text-muted-foreground" aria-label="只读挂载" />}
        <span className="min-w-0 flex-1 truncate">{node.name}</span>
        {!node.is_dir && node.size > 0 && (
          <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70 group-hover:invisible">{formatBytes(node.size)}</span>
        )}
      </button>
      {/* Row actions: hover-only, never on read-only subtrees. */}
      <span className="absolute right-1 top-0 hidden items-center gap-0.5 group-hover:flex" data-vfs-actions={node.path}>
        {!node.is_dir && (
          <button type="button" aria-label={`下载 ${node.name}`} title="下载" onClick={() => void download()} className="grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground">
            <Download className="size-3" />
          </button>
        )}
        {!node.readOnly && (
          <>
            <button type="button" aria-label={`重命名 ${node.name}`} title="重命名" onClick={() => void rename()} className="grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground">
              <Pencil className="size-3" />
            </button>
            <button type="button" aria-label={`删除 ${node.name}`} title="删除" onClick={() => void remove()} className="grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground">
              <Trash2 className="size-3" />
            </button>
          </>
        )}
      </span>
      {error && (
        <p role="alert" className="px-2 py-0.5 text-[11px] text-destructive" style={{ paddingLeft: `${depth * 12 + 8}px` }}>{error}</p>
      )}
    </div>
  );
}

function Branch({
  path,
  depth,
  activeDir,
  onSelect,
  onMutated,
}: {
  path: string;
  depth: number;
  activeDir: string;
  onSelect: (node: TreeNode) => void;
  onMutated: (parent: string) => void;
}) {
  const state = getVfsTreeState();
  const children = state.children.get(path);
  const error = state.errors.get(path);
  const loading = state.loading.has(path);
  if (error) {
    return <p className="px-2 py-1 text-[11px] text-destructive" role="alert" style={{ paddingLeft: `${depth * 12 + 4}px` }}>{error}</p>;
  }
  if (!children) {
    return loading ? (
      <p className="px-2 py-0.5 text-[11px] text-muted-foreground" style={{ paddingLeft: `${depth * 12 + 4}px` }}>加载中…</p>
    ) : null;
  }
  if (children.length === 0) {
    return <p className="px-2 py-0.5 text-[11px] text-muted-foreground" style={{ paddingLeft: `${depth * 12 + 4}px` }}>(空)</p>;
  }
  return (
    <>
      {children.map((node) => (
        <div key={node.path}>
          <Row node={node} depth={depth} active={node.path === activeDir} onSelect={onSelect} onMutated={onMutated} />
          {node.is_dir && state.expanded.has(node.path) && (
            <Branch path={node.path} depth={depth + 1} activeDir={activeDir} onSelect={onSelect} onMutated={onMutated} />
          )}
        </div>
      ))}
    </>
  );
}

export function VfsSidebar() {
  useSyncExternalStore(subscribeVfsTree, getVfsTreeState);
  const [activeDir, setActiveDir] = useState("/");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    void bootstrapTree();
  }, []);

  const reload = (dir: string) => {
    void reloadSubtree(dir);
  };

  const newFolder = async () => {
    const name = window.prompt(`在 ${activeDir} 下新建文件夹:`, "");
    if (!name) return;
    const path = activeDir === "/" ? `/${name}` : `${activeDir.replace(/\/+$/, "")}/${name}`;
    setBusy(true);
    setError("");
    try {
      await mkdir(path);
      reload(activeDir);
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const newFile = async () => {
    const name = window.prompt(`在 ${activeDir} 下新建文件:`, "");
    if (!name) return;
    const path = activeDir === "/" ? `/${name}` : `${activeDir.replace(/\/+$/, "")}/${name}`;
    setBusy(true);
    setError("");
    try {
      await saveTextFile(path, "", null);
      reload(activeDir);
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const upload = async () => {
    const source = await pickUploadSource();
    if (!source) return;
    setBusy(true);
    setError("");
    try {
      await uploadToDir(activeDir, source);
      reload(activeDir);
    } catch (cause) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const writable = !readOnlyCover(activeDir);

  return (
    <div role="group" aria-label="VFS 文件浏览" data-vfs-sidebar data-vfs-active-dir={activeDir} className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b px-2">
        <Server className="size-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">VFS</span>
        <span className="min-w-0 flex-1 truncate text-[10px] text-muted-foreground/70" title={activeDir}>{activeDir}</span>
        <Button size="icon-sm" variant="ghost" aria-label="新建文件夹" title={`在 ${activeDir} 新建文件夹`} disabled={busy || !writable} onClick={() => void newFolder()}>
          <FolderPlus className="size-3.5" />
        </Button>
        <Button size="icon-sm" variant="ghost" aria-label="新建文件" title={`在 ${activeDir} 新建文件`} disabled={busy || !writable} onClick={() => void newFile()}>
          <FilePlus2 className="size-3.5" />
        </Button>
        <Button size="icon-sm" variant="ghost" aria-label="上传文件" title={`上传到 ${activeDir}`} disabled={busy || !writable} onClick={() => void upload()}>
          <Upload className="size-3.5" />
        </Button>
        <Button
          size="icon-sm"
          variant="ghost"
          className="ml-auto"
          aria-label="重新加载"
          title="重新加载"
          onClick={() => {
            resetTree();
            void bootstrapTree();
          }}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      {!writable && (
        <p className="border-b bg-muted/40 px-2 py-1 text-[10px] text-muted-foreground">
          <Lock className="mr-1 inline size-3" />{activeDir} 位于只读挂载,仅可浏览与下载。
        </p>
      )}
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-2 py-1 text-[11px] text-destructive">{error}</div>}
      <div role="tree" aria-label="VFS 目录树" className="min-h-0 flex-1 overflow-y-auto p-1">
        <Branch
          path="/"
          depth={0}
          activeDir={activeDir}
          onSelect={(node) => setActiveDir(node.path)}
          onMutated={reload}
        />
      </div>
    </div>
  );
}
