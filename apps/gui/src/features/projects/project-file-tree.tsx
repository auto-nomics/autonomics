import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileCode2, Folder, FolderOpen } from "lucide-react";

type FileNode = { name: string; path: string; children: Map<string, FileNode> | null };

function buildTree(files: string[], directories: string[]): FileNode[] {
  const root = new Map<string, FileNode>();
  for (const { path, isDirectory } of [...directories.map(path => ({ path, isDirectory: true })), ...files.map(path => ({ path, isDirectory: false }))]) {
    const parts = path.split("/");
    let children = root;
    for (let index = 0; index < parts.length; index++) {
      const name = parts[index];
      const directory = isDirectory || index < parts.length - 1;
      let node = children.get(name);
      if (!node) {
        node = { name, path: parts.slice(0, index + 1).join("/"), children: directory ? new Map() : null };
        children.set(name, node);
      }
      if (node.children) children = node.children;
    }
  }
  return sortedNodes(root);
}

function sortedNodes(nodes: Map<string, FileNode>): FileNode[] {
  return [...nodes.values()].sort((a, b) => Number(Boolean(b.children)) - Number(Boolean(a.children)) || a.name.localeCompare(b.name, undefined, { numeric: true }));
}

export function ProjectFileTree({ files, directories, selected, disabled, directoryMove, onOpen, onSelectDirectory }: { files: string[]; directories: string[]; selected: string; disabled: boolean; directoryMove?: { from: string; to: string; id: number }; onOpen: (path: string) => void; onSelectDirectory: (path: string) => void }) {
  const nodes = useMemo(() => buildTree(files, directories), [files, directories]);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    if (!directoryMove) return;
    const { from, to } = directoryMove;
    setExpanded(current => new Set([...current].map(path => path === from || path.startsWith(`${from}/`) ? to + path.slice(from.length) : path)));
  }, [directoryMove]);

  function toggle(path: string) {
    setExpanded(current => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  }

  function renderNodes(items: FileNode[], depth: number) {
    return <ul className="list-none">{items.map(node => {
      const isDirectory = node.children !== null;
      const open = isDirectory && expanded.has(node.path);
      return <li key={node.path}>
        <button type="button" disabled={disabled} title={node.path} aria-expanded={isDirectory ? open : undefined} aria-current={selected === node.path ? "true" : undefined}
          className={`flex min-h-7 w-full items-center gap-1 text-left text-xs hover:bg-accent ${selected === node.path ? "bg-accent font-medium" : ""}`}
          style={{ paddingLeft: `${8 + depth * 16}px`, paddingRight: 8 }}
          onClick={() => { if (isDirectory) { onSelectDirectory(node.path); toggle(node.path); } else onOpen(node.path); }}>
          {isDirectory ? open ? <ChevronDown className="size-3.5 shrink-0" /> : <ChevronRight className="size-3.5 shrink-0" /> : <span className="w-3.5 shrink-0" />}
          {isDirectory ? open ? <FolderOpen className="size-3.5 shrink-0" /> : <Folder className="size-3.5 shrink-0" /> : <FileCode2 className="size-3.5 shrink-0" />}
          <span className="min-w-0 truncate">{node.name}</span>
        </button>
        {isDirectory && open && renderNodes(sortedNodes(node.children!), depth + 1)}
      </li>;
    })}</ul>;
  }

  return <nav aria-label="项目文件">{renderNodes(nodes, 0)}</nav>;
}
