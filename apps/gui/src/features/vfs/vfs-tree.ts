/**
 * Lazy VFS directory tree state: one entry list per expanded directory,
 * fetched through the paginated `/vfs/list` API. Mount definitions are
 * prefetched so read-only subtrees can carry a badge before their
 * entries load. Module-level external store (workbench-store pattern)
 * so the sidebar, editor actions and future file-management commands all
 * observe the same tree.
 */

import { listDirAll, listMounts, type VfsEntry, type VfsMount } from "./vfs-api";
import { fetchComputeStatus } from "@/features/hpc/api";
import type { ComputeStatus } from "@/features/hpc/types";

export type TreeNode = VfsEntry & { readOnly?: boolean };

/** One compute target's slice of the VFS namespace. Local targets own the
 *  `/` root; remote targets (ssh/slurm) own the subtrees their data
 *  domains project via `vfs_prefix` (always read-only mounts). */
export type ComputeGroup = {
  id: string;
  name: string;
  backend: "local" | "slurm" | "ssh";
  /** VFS subtree roots rendered under this group's header. */
  roots: string[];
  /** Top-level VFS paths hidden from the local `/` listing because a
   *  remote group owns them (prevents duplication between groups). */
  hidden?: string[];
};

export type VfsTreeState = {
  /** dir path → sorted children (dirs first). */
  children: Map<string, TreeNode[]>;
  /** Expanded directory paths. */
  expanded: Set<string>;
  /** Directories whose fetch failed → message per path. */
  errors: Map<string, string>;
  /** In-flight directory loads (prevents duplicate fetches). */
  loading: Set<string>;
  /** Declarative mounts (read-only badge source). */
  mounts: VfsMount[];
  /** Compute-target groups (one tree slice per target). */
  groups: ComputeGroup[];
  /** Bumped on every mutation (useSyncExternalStore identity). */
  revision: number;
};

const LOCAL_FALLBACK: ComputeGroup = { id: "local", name: "本地", backend: "local", roots: ["/"] };

let state: VfsTreeState = {
  children: new Map(),
  expanded: new Set(),
  errors: new Map(),
  loading: new Set(),
  mounts: [],
  groups: [LOCAL_FALLBACK],
  revision: 0,
};

const listeners = new Set<() => void>();

function commit(next: Partial<VfsTreeState>): void {
  state = { ...state, ...next, revision: state.revision + 1 };
  for (const listener of listeners) listener();
}

export function subscribeVfsTree(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getVfsTreeState(): VfsTreeState {
  return state;
}

/** Longest read-only mount covering `path`, if any. */
export function readOnlyCover(path: string): boolean {
  const address = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/.exec(path);
  if (address) {
    const [, domain, encoded] = address;
    let rest = encoded;
    if (domain !== "vfs") {
      try { rest = decodeURIComponent(encoded); } catch { return true; }
    }
    const primary = state.mounts.find((mount) => mount.domain === domain);
    if (domain === "vfs") path = `/${rest.replace(/^\/+/, "")}`;
    else if (primary) path = `${primary.path.replace(/\/+$/, "")}/${rest.replace(/^\/+/, "")}`;
    else if (domain === "local") path = `/${rest.replace(/^\/+/, "")}`;
    else return true;
  }
  const covering = state.mounts
    .filter((mount) => path === mount.path || path.startsWith(mount.path.endsWith("/") ? mount.path : `${mount.path}/`))
    .sort((a, b) => b.path.length - a.path.length)[0];
  return covering?.read_only ?? false;
}

function sortEntries(entries: VfsEntry[]): TreeNode[] {
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return a.name.localeCompare(b.name, undefined, { numeric: true });
  }).map((entry) => ({ ...entry, readOnly: readOnlyCover(entry.path) || undefined }));
}

/** Fetch a directory's children (all pages) into the store. */
export async function loadDir(path: string): Promise<void> {
  if (state.loading.has(path) || state.children.has(path)) return;
  const errors = new Map(state.errors);
  errors.delete(path);
  commit({ loading: new Set(state.loading).add(path), errors });
  try {
    const entries = await listDirAll(path);
    commit({
      children: new Map(state.children).set(path, sortEntries(entries)),
      loading: new Set([...state.loading].filter((item) => item !== path)),
    });
  } catch (error) {
    commit({
      loading: new Set([...state.loading].filter((item) => item !== path)),
      errors: new Map(state.errors).set(path, String(error)),
    });
  }
}

/** (Re)fetch a directory, replacing cached children. */
export async function refreshDir(path: string): Promise<void> {
  const children = new Map(state.children);
  children.delete(path);
  commit({ children });
  await loadDir(path);
}

/** Drop a directory and its descendants' caches (after mutations). */
export function invalidateDir(path: string): void {
  const prefix = path.endsWith("/") ? path : `${path}/`;
  const children = new Map(state.children);
  for (const key of children.keys()) {
    if (key === path || key.startsWith(prefix)) children.delete(key);
  }
  commit({ children });
}

/** Invalidate `path` and its descendants, then reload every directory
 *  that is (or was) cached or expanded below it — mutations must not
 *  leave expanded folders stuck on a cleared cache. */
export async function reloadSubtree(path: string): Promise<void> {
  const prefix = path.endsWith("/") ? path : `${path}/`;
  const covers = (key: string) => key === path || key.startsWith(prefix);
  const affected = [...state.children.keys()].filter(covers);
  invalidateDir(path);
  const targets = new Set<string>([
    ...affected,
    ...[...state.expanded].filter(covers),
  ]);
  if (covers("/")) targets.add("/");
  await Promise.all([...targets].map((dir) => loadDir(dir)));
}

export function toggleDir(path: string): void {
  const expanded = new Set(state.expanded);
  if (expanded.has(path)) {
    expanded.delete(path);
    commit({ expanded });
  } else {
    expanded.add(path);
    commit({ expanded });
    void loadDir(path);
  }
}

export function expandDir(path: string): void {
  if (state.expanded.has(path)) return;
  commit({ expanded: new Set(state.expanded).add(path) });
  void loadDir(path);
}

/** Build compute-target groups from the compute registry. Local targets
 *  keep the whole `/` namespace; each remote target's data domains
 *  contribute their `vfs_prefix` (and retained prefix) as group roots,
 *  which are hidden from the local top level to avoid duplication. */
function buildGroups(status: ComputeStatus | null): ComputeGroup[] {
  if (!status) return [LOCAL_FALLBACK];
  const groups: ComputeGroup[] = [];
  const hidden: string[] = [];
  for (const target of status.state.targets) {
    if (target.enabled === false) continue;
    const backend = target.backend?.type ?? "local";
    if (backend === "local") {
      groups.push({ id: target.id, name: target.name || "本地", backend: "local", roots: ["/"] });
      continue;
    }
    const roots: string[] = [];
    for (const domain of target.data_domains ?? []) {
      for (const prefix of [domain.vfs_prefix, domain.retained_vfs_prefix]) {
        if (!prefix) continue;
        roots.push(prefix);
        hidden.push(prefix);
      }
    }
    if (roots.length === 0) continue;
    groups.push({ id: target.id, name: target.name || target.id, backend, roots });
  }
  if (groups.filter((group) => group.backend === "local").length === 0) {
    groups.unshift(LOCAL_FALLBACK);
  }
  for (const group of groups) {
    if (group.backend === "local") group.hidden = hidden;
  }
  return groups;
}

/** Prefetch mounts (read-only badges), compute-target groups, and the
 *  group root listings. */
export async function bootstrapTree(): Promise<void> {
  if (state.children.size > 0 || state.loading.size > 0) return;
  let status: ComputeStatus | null = null;
  try {
    status = await fetchComputeStatus();
  } catch {
    /* compute registry unavailable — fall back to a plain local root */
  }
  const groups = buildGroups(status);
  try {
    const { mounts } = await listMounts();
    commit({ mounts, groups });
  } catch {
    /* mounts are cosmetic; the tree still loads */
    commit({ groups });
  }
  const roots = new Set(groups.flatMap((group) => group.roots));
  await Promise.all([...roots].map((root) => loadDir(root)));
  for (const root of roots) expandDir(root);
}

export function resetTree(): void {
  state = {
    children: new Map(),
    expanded: new Set(),
    errors: new Map(),
    loading: new Set(),
    mounts: [],
    groups: [LOCAL_FALLBACK],
    revision: state.revision + 1,
  };
  for (const listener of listeners) listener();
}
