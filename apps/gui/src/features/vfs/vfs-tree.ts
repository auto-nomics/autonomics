/**
 * Lazy VFS directory tree state: one entry list per expanded directory,
 * fetched through the paginated `/vfs/list` API. Mount definitions are
 * prefetched so read-only subtrees can carry a badge before their
 * entries load. Module-level external store (workbench-store pattern)
 * so the sidebar, editor actions and future file-management commands all
 * observe the same tree.
 */

import { listDirAll, listMounts, type VfsEntry, type VfsMount } from "./vfs-api";

export type TreeNode = VfsEntry & { readOnly?: boolean };

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
  /** Bumped on every mutation (useSyncExternalStore identity). */
  revision: number;
};

let state: VfsTreeState = {
  children: new Map(),
  expanded: new Set(),
  errors: new Map(),
  loading: new Set(),
  mounts: [],
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
  return state.mounts.some(
    (mount) =>
      mount.read_only &&
      (path === mount.path || path.startsWith(mount.path.endsWith("/") ? mount.path : `${mount.path}/`)),
  );
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

/** Prefetch mounts (read-only badges) and the root listing. */
export async function bootstrapTree(): Promise<void> {
  if (state.children.size > 0 || state.loading.size > 0) return;
  try {
    const { mounts } = await listMounts();
    commit({ mounts });
  } catch {
    /* mounts are cosmetic; the tree still loads */
  }
  await loadDir("/");
  expandDir("/");
}

export function resetTree(): void {
  state = {
    children: new Map(),
    expanded: new Set(),
    errors: new Map(),
    loading: new Set(),
    mounts: [],
    revision: state.revision + 1,
  };
  for (const listener of listeners) listener();
}
