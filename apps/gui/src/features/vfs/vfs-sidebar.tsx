/**
 * VFS browser sidebar: a lazy directory tree over the whole virtual
 * namespace. Directories expand on click (fetched page by page); text
 * files open a `vfs-file` editor tab; images open a preview tab;
 * everything else offers a download. Read-only mounts carry a lock
 * badge. File-management actions (upload/new/rename/delete) arrive with
 * the write endpoints.
 */

import { useEffect } from "react";
import { useSyncExternalStore } from "react";
import { ChevronDown, ChevronRight, Folder, FolderOpen, Lock, RefreshCw, Server } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { bootstrapTree, getVfsTreeState, resetTree, subscribeVfsTree, toggleDir, type TreeNode } from "./vfs-tree";
import { formatBytes } from "./vfs-api";
import { openOrDownloadVfsFile } from "./vfs-file";

function Row({ node, depth }: { node: TreeNode; depth: number }) {
  const isOpen = getVfsTreeState().expanded.has(node.path);
  return (
    <button
      type="button"
      role={node.is_dir ? "treeitem" : undefined}
      aria-expanded={node.is_dir ? isOpen : undefined}
      aria-label={node.path}
      title={`${node.path}${node.is_dir ? "" : ` · ${formatBytes(node.size)}`}`}
      data-vfs-path={node.path}
      data-vfs-dir={node.is_dir || undefined}
      onClick={() => (node.is_dir ? toggleDir(node.path) : void openOrDownloadVfsFile(node.path, node.readOnly))}
      className={cn(
        "flex w-full items-center gap-1 rounded-sm px-1 py-0.5 text-left text-xs hover:bg-accent",
        !node.is_dir && "cursor-pointer",
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
        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70">{formatBytes(node.size)}</span>
      )}
    </button>
  );
}

function Branch({ path, depth }: { path: string; depth: number }) {
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
          <Row node={node} depth={depth} />
          {node.is_dir && state.expanded.has(node.path) && <Branch path={node.path} depth={depth + 1} />}
        </div>
      ))}
    </>
  );
}

export function VfsSidebar() {
  useSyncExternalStore(subscribeVfsTree, getVfsTreeState);

  useEffect(() => {
    void bootstrapTree();
  }, []);

  return (
    <div role="group" aria-label="VFS 文件浏览" data-vfs-sidebar className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b px-2">
        <Server className="size-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">VFS</span>
        <Button
          size="icon-sm"
          variant="ghost"
          className="ml-auto"
          aria-label="重新加载根目录"
          title="重新加载"
          onClick={() => {
            resetTree();
            void bootstrapTree();
          }}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      <div role="tree" aria-label="VFS 目录树" className="min-h-0 flex-1 overflow-y-auto p-1">
        <Branch path="/" depth={0} />
      </div>
    </div>
  );
}
