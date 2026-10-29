import type { BibCollection } from "@/types";

export type CollectionRow = { collection: BibCollection; depth: number; hasChildren: boolean };
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

export function collectionPath(collections: BibCollection[], id: string): string {
  const byId = new Map(collections.map(c => [c.id, c]));
  const names: string[] = [];
  const seen = new Set<string>();
  let current = byId.get(id);
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    names.unshift(current.name);
    current = current.parent_id ? byId.get(current.parent_id) : undefined;
  }
  return names.join(" / ");
}

export function collectionDescendants(collections: BibCollection[], id: string): Set<string> {
  const result = new Set([id]);
  const children = new Map<string, string[]>();
  for (const c of collections) if (c.parent_id) children.set(c.parent_id, [...(children.get(c.parent_id) ?? []), c.id]);
  const queue = [id];
  for (let at = 0; at < queue.length; at++) {
    for (const child of children.get(queue[at]) ?? []) if (!result.has(child)) { result.add(child); queue.push(child); }
  }
  return result;
}

/** Names matching the filter plus every ancestor that keeps them reachable;
 *  expanded additionally auto-reveals nested matches while the filter is active.
 *  Returns null when the query is empty, meaning no filtering. */
export function collectionFilter(collections: BibCollection[], query: string): { visible: Set<string>; expanded: Set<string> } | null {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return null;
  const byId = new Map(collections.map(c => [c.id, c]));
  const visible = new Set<string>();
  const expanded = new Set<string>();
  for (const c of collections) {
    if (!c.name.toLocaleLowerCase().includes(needle)) continue;
    let current: BibCollection | undefined = c;
    while (current && !visible.has(current.id)) {
      visible.add(current.id);
      expanded.add(current.id);
      current = current.parent_id ? byId.get(current.parent_id) : undefined;
    }
  }
  return { visible, expanded };
}

export function collectionRows(collections: BibCollection[], expanded: Set<string>, deleted = false): CollectionRow[] {
  const nodes = collections.filter(c => Boolean(c.deleted) === deleted);
  const ids = new Set(nodes.map(c => c.id));
  const children = new Map<string | null, BibCollection[]>();
  for (const c of nodes) {
    const parent = c.parent_id && ids.has(c.parent_id) ? c.parent_id : null;
    children.set(parent, [...(children.get(parent) ?? []), c]);
  }
  for (const group of children.values()) group.sort((a, b) => collator.compare(a.name, b.name) || a.id.localeCompare(b.id));
  const rows: CollectionRow[] = [];
  const seen = new Set<string>();
  const stack = (children.get(null) ?? []).map(collection => ({ collection, depth: 1 })).reverse();
  while (stack.length) {
    const { collection, depth } = stack.pop()!;
    if (seen.has(collection.id)) continue;
    seen.add(collection.id);
    const kids = children.get(collection.id) ?? [];
    rows.push({ collection, depth, hasChildren: kids.length > 0 });
    if (expanded.has(collection.id)) for (const child of [...kids].reverse()) stack.push({ collection: child, depth: depth + 1 });
  }
  return rows;
}
