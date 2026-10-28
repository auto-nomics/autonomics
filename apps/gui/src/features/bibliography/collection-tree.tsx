import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, Folder, Library, Inbox, Plus, Trash2 } from "lucide-react";
import type { BibCollection } from "@/types";
import { collectionDescendants, collectionPath, collectionRows } from "./collection-tree-model";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Button } from "@/components/ui/button";
import { Dialog as ConfirmDialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export type CollectionAction = { method: "POST" | "PATCH" | "DELETE"; path: string; body?: unknown };
type Dialog = { kind: "create" | "rename" | "move" | "trash" | "erase"; id: string | null };
type Props = {
  collections: BibCollection[];
  selected: string | null;
  total: number;
  onSelect: (id: string | null) => void;
  onAction: (action: CollectionAction) => Promise<void>;
};
const storageKey = "autonomics.collections.expanded.v1";
function readExpanded(): Set<string> {
  try { return new Set(JSON.parse(localStorage.getItem(storageKey) ?? "[]")); } catch { return new Set(); }
}

export function CollectionTree({ collections, selected, total, onSelect, onAction }: Props) {
  const [expanded, setExpanded] = useState(readExpanded);
  const [focused, setFocused] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [name, setName] = useState("");
  const [parent, setParent] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [emptyTrashOpen, setEmptyTrashOpen] = useState(false);
  const tree = useRef<HTMLDivElement>(null);
  const modal = useRef<HTMLDialogElement>(null);
  const trashNode: BibCollection = { id: "@trash", name: "Trash", article_ids: [], direct_article_count: collections.filter(c => c.deleted).length, deleted: true };
  const rows = [...collectionRows(collections, expanded), { collection: trashNode, depth: 1, hasChildren: true }, ...(expanded.has("@trash") ? collectionRows(collections, expanded, true).map(row => ({ ...row, depth: row.depth + 1 })) : [])];
  const focusId = rows.some(row => row.collection.id === focused) ? focused : rows.some(row => row.collection.id === selected) ? selected : rows[0]?.collection.id;
  const pending = collections.find(c => c.id === dialog?.id);
  const excluded = dialog?.id ? collectionDescendants(collections, dialog.id) : new Set<string>();

  useEffect(() => { try { localStorage.setItem(storageKey, JSON.stringify([...expanded])); } catch { /* preference storage may be unavailable */ } }, [expanded]);
  useEffect(() => {
    if (!selected) return;
    const byId = new Map(collections.map(c => [c.id, c]));
    const ancestors: string[] = [];
    const seen = new Set<string>();
    let current = byId.get(selected);
    while (current?.parent_id && !seen.has(current.parent_id)) {
      seen.add(current.parent_id); ancestors.push(current.parent_id); current = byId.get(current.parent_id);
    }
    if (ancestors.length) setExpanded(prev => new Set([...prev, ...ancestors]));
  }, [selected, collections]);
  useEffect(() => { if (dialog && !modal.current?.open) modal.current?.showModal(); }, [dialog]);

  function toggle(id: string) {
    if (expanded.has(id) && focused && collectionDescendants(collections, id).has(focused)) focus(id);
    setExpanded(prev => { const next = new Set(prev); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  }
  function open(kind: Dialog["kind"], id: string | null) {
    const c = collections.find(c => c.id === id);
    setDialog({ kind, id }); setName(kind === "rename" ? c?.name ?? "" : "");
    setParent(kind === "create" ? id ?? "" : c?.parent_id ?? ""); setError("");
  }
  function focus(id: string) {
    setFocused(id);
    const node = Array.from(tree.current?.querySelectorAll<HTMLElement>("[data-collection-id]") ?? []).find(n => n.dataset.collectionId === id);
    node?.focus();
  }
  async function action(input: CollectionAction) {
    setBusy(true); setError("");
    try { await onAction(input); } catch (e) { setError(String(e)); throw e; } finally { setBusy(false); }
  }
  async function submit(event: React.FormEvent) {
    event.preventDefault(); if (!dialog) return;
    const path = `/bib/collections${dialog.kind === "create" ? "" : `/${encodeURIComponent(dialog.id!)}`}`;
    let input: CollectionAction;
    if (dialog.kind === "create") input = { method: "POST", path, body: { name: name.trim(), parent_id: parent || null } };
    else if (dialog.kind === "rename") input = { method: "PATCH", path, body: { name: name.trim() } };
    else if (dialog.kind === "move") input = { method: "PATCH", path, body: { parent_id: parent || null } };
    else input = { method: "DELETE", path: `${path}${dialog.kind === "erase" ? "?permanent=true" : ""}` };
    try { await action(input); modal.current?.close(); setDialog(null); } catch { /* keep the dialog and input */ }
  }
  async function emptyTrash() {
    const trashed = collections.filter(c => c.deleted);
    const ids = new Set(trashed.map(c => c.id));
    const roots = trashed.filter(c => !c.parent_id || !ids.has(c.parent_id));
    if (!roots.length) return;
    setBusy(true); setError("");
    try {
      for (const root of roots) await onAction({ method: "DELETE", path: `/bib/collections/${encodeURIComponent(root.id)}?permanent=true` });
      setEmptyTrashOpen(false);
    } catch (e) { setError(`Could not empty trash: ${String(e)}`); }
    finally { setBusy(false); }
  }
  async function drop(event: React.DragEvent, parentId: string | null) {
    event.preventDefault(); event.stopPropagation();
    const id = event.dataTransfer.getData("application/x-autonomics-collection");
    if (id && !collectionDescendants(collections, id).has(parentId ?? "")) {
      try { await action({ method: "PATCH", path: `/bib/collections/${encodeURIComponent(id)}`, body: { parent_id: parentId } });
        if (parentId) setExpanded(prev => new Set([...prev, parentId]));
      } catch { /* error is visible */ }
    }
    const articleId = event.dataTransfer.getData("application/x-autonomics-article");
    if (articleId && parentId) try { await action({ method: "POST", path: `/bib/collections/${encodeURIComponent(parentId)}/articles`, body: { article_id: articleId } }); } catch { /* error is visible */ }
  }
  return <>
    <div className="flex items-center justify-between px-2 py-1">
      <span className="text-xs text-muted-foreground">Collections</span>
      <Button size="icon-xs" variant="ghost" aria-label="New collection" onClick={() => open("create", null)}><Plus /></Button>
    </div>
    <button type="button" className={cn("flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs", !selected && "bg-accent")} onClick={() => onSelect(null)} onDragOver={e => e.preventDefault()} onDrop={e => void drop(e, null)}><Library className="size-4" />All Articles<span className="ml-auto">{total}</span></button>
    <button type="button" className={cn("flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs", selected === "@unfiled" && "bg-accent")} onClick={() => onSelect("@unfiled")}><Inbox className="size-4" />Unfiled</button>
    <div ref={tree} role="tree" aria-label="Collections" className="py-1">
      {rows.map(({ collection: c, depth, hasChildren }, index) => c.id === "@trash" ? <ContextMenu key={c.id}><ContextMenuTrigger render={<div />}><div role="treeitem" data-collection-id={c.id} aria-label="Trash" aria-level={1} aria-expanded={expanded.has(c.id)} tabIndex={focusId === c.id ? 0 : -1} onFocus={() => setFocused(c.id)} onKeyDown={event => { if (["Enter", " ", "ArrowRight", "ArrowLeft"].includes(event.key)) { event.preventDefault(); toggle(c.id); } }}><button type="button" className="mt-2 flex w-full items-center gap-2 border-t px-2 py-2 text-left text-xs" aria-label="Toggle collection trash" onClick={() => toggle(c.id)}>{expanded.has(c.id) ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}<Trash2 className="size-3.5" />Trash<span className="ml-auto">{c.direct_article_count}</span></button></div></ContextMenuTrigger><ContextMenuContent><ContextMenuItem variant="destructive" disabled={busy || !c.direct_article_count} onClick={() => { setError(""); setEmptyTrashOpen(true); }}>Empty trash…</ContextMenuItem></ContextMenuContent></ContextMenu> : <ContextMenu key={c.id}>
        <ContextMenuTrigger render={<div />}>
          <div role="treeitem" data-collection-id={c.id} aria-label={c.name} aria-level={depth} aria-selected={selected === c.id} aria-expanded={hasChildren ? expanded.has(c.id) : undefined}
            tabIndex={focusId === c.id ? 0 : -1}
            className={cn("flex min-h-8 cursor-default items-center gap-1 rounded pr-2 text-xs outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring", selected === c.id && "bg-accent", c.deleted && "text-muted-foreground")}
            style={{ paddingLeft: `${(depth - 1) * 16 + 4}px` }}
            onFocus={() => setFocused(c.id)} onClick={() => { if (!c.deleted) onSelect(c.id); }}
            onKeyDown={e => {
              let target: string | undefined;
              if (e.key === "ArrowDown") target = rows[index + 1]?.collection.id;
              if (e.key === "ArrowUp") target = rows[index - 1]?.collection.id;
              if (e.key === "Home") target = rows[0]?.collection.id;
              if (e.key === "End") target = rows.at(-1)?.collection.id;
              if (e.key === "ArrowRight") { if (hasChildren && !expanded.has(c.id)) toggle(c.id); else if (hasChildren) target = rows[index + 1]?.collection.id; }
              if (e.key === "ArrowLeft") { if (expanded.has(c.id)) toggle(c.id); else target = c.parent_id ?? undefined; }
              if (e.key === "Enter" && !c.deleted) onSelect(c.id);
              if (e.key === "F2" && !c.deleted) open("rename", c.id);
              if (e.key === "Delete") open(c.deleted ? "erase" : "trash", c.id);
              if (["ArrowDown", "ArrowUp", "ArrowRight", "ArrowLeft", "Home", "End", "F2", "Delete"].includes(e.key)) e.preventDefault();
              if (target) focus(target);
            }}
            draggable={!c.deleted} onDragStart={e => { e.dataTransfer.setData("application/x-autonomics-collection", c.id); e.dataTransfer.effectAllowed = "move"; }}
            onDragOver={e => { if (!c.deleted) e.preventDefault(); }} onDrop={e => { if (!c.deleted) void drop(e, c.id); }}>
            <button type="button" tabIndex={-1} aria-label={`${expanded.has(c.id) ? "Collapse" : "Expand"} ${c.name}`} className="flex size-5 shrink-0 items-center justify-center" onClick={e => { e.stopPropagation(); if (hasChildren) toggle(c.id); }}>{hasChildren ? expanded.has(c.id) ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" /> : null}</button>
            {c.deleted ? <Trash2 className="size-3.5 shrink-0" /> : <Folder className="size-3.5 shrink-0" />}<span className="truncate" title={collectionPath(collections, c.id)}>{c.name}</span><span className="ml-auto text-muted-foreground">{c.direct_article_count ?? c.article_ids?.length ?? 0}</span>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          {c.deleted ? <><ContextMenuItem disabled={busy} onClick={() => void action({ method: "POST", path: `/bib/collections/${encodeURIComponent(c.id)}/restore` }).catch(() => {})}>Restore collection</ContextMenuItem><ContextMenuItem onClick={() => open("erase", c.id)}>Delete permanently…</ContextMenuItem></> : <>
            <ContextMenuItem onClick={() => open("create", c.id)}>New subcollection…</ContextMenuItem>
            <ContextMenuItem onClick={() => open("rename", c.id)}>Rename collection…</ContextMenuItem>
            <ContextMenuItem onClick={() => open("move", c.id)}>Move collection…</ContextMenuItem>
            <ContextMenuSeparator /><ContextMenuItem onClick={() => open("trash", c.id)}>Move collection to trash…</ContextMenuItem>
          </>}
        </ContextMenuContent>
      </ContextMenu>)}
    </div>

    <ConfirmDialog open={emptyTrashOpen} onOpenChange={(open) => { if (!busy) setEmptyTrashOpen(open); }}>
      <DialogContent role="alertdialog" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Empty trash?</DialogTitle>
          <DialogDescription>Permanently delete all {collections.filter(c => c.deleted).length} trashed collections? Articles, attachments and notes remain in your library.</DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => setEmptyTrashOpen(false)}>Cancel</Button>
          <Button variant="destructive" disabled={busy} onClick={() => void emptyTrash()}>{busy ? "Deleting…" : "Empty trash"}</Button>
        </DialogFooter>
      </DialogContent>
    </ConfirmDialog>
    {error && !dialog && !emptyTrashOpen && <p role="alert" className="p-2 text-xs text-destructive">{error}</p>}
    {dialog && <dialog ref={modal} aria-labelledby="collection-dialog-title" onCancel={e => { if (busy) e.preventDefault(); else setDialog(null); }} className="fixed inset-0 m-auto w-[min(90vw,420px)] rounded-lg border bg-card p-5 text-foreground shadow-xl backdrop:bg-black/50">
      <form onSubmit={submit} className="space-y-4">
        <h2 id="collection-dialog-title" className="font-semibold">{({ create: "New collection", rename: "Rename collection", move: "Move collection", trash: "Move collection to trash?", erase: "Delete collection permanently?" })[dialog.kind]}</h2>
        {(dialog.kind === "create" || dialog.kind === "rename") && <label className="block text-sm">Name<Input autoFocus value={name} onChange={e => setName(e.target.value)} required /></label>}
        {(dialog.kind === "create" || dialog.kind === "move") && <label className="block text-sm">Parent collection<select autoFocus={dialog.kind === "move"} className="mt-1 block w-full rounded border bg-background p-2" aria-label="Parent collection" value={parent} onChange={e => setParent(e.target.value)}><option value="">Library root</option>{collections.filter(c => !c.deleted && (dialog.kind === "create" || !excluded.has(c.id))).map(c => <option key={c.id} value={c.id}>{collectionPath(collections, c.id)}</option>)}</select></label>}
        {(dialog.kind === "trash" || dialog.kind === "erase") && <p className="text-sm">{pending?.name} and {Math.max(0, excluded.size - 1)} subcollections will be {dialog.kind === "erase" ? "permanently deleted" : "moved to trash"}. Articles, attachments and notes remain in your library.</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <div className="flex justify-end gap-2"><Button type="button" variant="outline" disabled={busy} onClick={() => { modal.current?.close(); setDialog(null); }}>Cancel</Button><Button type="submit" disabled={busy || ((dialog.kind === "create" || dialog.kind === "rename") && !name.trim())}>{busy ? "Saving…" : dialog.kind === "trash" ? "Move to trash" : dialog.kind === "erase" ? "Delete permanently" : "Save"}</Button></div>
      </form>
    </dialog>}
  </>;
}
