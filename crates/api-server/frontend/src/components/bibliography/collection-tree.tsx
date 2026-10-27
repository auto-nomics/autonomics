import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FolderOpen, Inbox, Library, Plus, Trash2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import type { Collection } from "@/lib/bib-api";

export type CollectionView = "all" | "unfiled" | `col:${string}`;

interface CollectionTreeProps {
  collections: Collection[];
  activeView: CollectionView;
  allCount: number;
  onSelect: (view: CollectionView) => void;
  onCreate: (name: string, parentId: string | null) => Promise<void>;
  onManage: (id: string, action: "rename" | "move" | "restore" | "erase", value?: string | null) => Promise<void>;
  recursive: boolean;
  onRecursive: (value: boolean) => void;
  onDelete: (id: string) => Promise<void>;
}

export function CollectionTree({
  collections,
  activeView,
  allCount,
  onSelect,
  onCreate,
  onDelete, onManage, recursive, onRecursive,
}: CollectionTreeProps) {
  const treeRef=useRef<HTMLDivElement>(null);
  const [focused,setFocused]=useState<string|null>(null);
  const focusRow=(id:string)=>{setFocused(id);Array.from(treeRef.current?.querySelectorAll<HTMLElement>("[data-collection-id]")??[]).find(node=>node.dataset.collectionId===id)?.focus();};
  const [parentId,setParentId]=useState<string|null>(null);
  const [moving,setMoving]=useState<string|null>(null);
  const [moveParent,setMoveParent]=useState("");
  const [actionError,setActionError]=useState("");
  const [expanded,setExpanded]=useState<Set<string>>(()=>{try{return new Set(JSON.parse(localStorage.getItem("bib.collections.expanded")??"[]"));}catch{return new Set();}});
  useEffect(()=>{localStorage.setItem("bib.collections.expanded",JSON.stringify([...expanded]));},[expanded]);
  useEffect(()=>{
    if(!activeView.startsWith("col:"))return;
    const parents:string[]=[];const seen=new Set<string>();let current=collections.find(c=>c.id===activeView.slice(4));
    while(current?.parent_id && !seen.has(current.parent_id)){seen.add(current.parent_id);parents.push(current.parent_id);current=collections.find(c=>c.id===current?.parent_id);}
    if(parents.length)setExpanded(prev=>new Set([...prev,...parents]));
  },[activeView,collections]);
  function toggle(id:string){setExpanded(prev=>{const next=new Set(prev);if(next.has(id))next.delete(id);else next.add(id);return next;});}
  const children=new Map<string|null,Collection[]>();const ids=new Set(collections.map(c=>c.id));
  for(const c of collections){const pid=c.parent_id && ids.has(c.parent_id) && Boolean(collections.find(p=>p.id===c.parent_id)?.deleted)===Boolean(c.deleted)?c.parent_id:c.deleted?"@trash":null;children.set(pid,[...(children.get(pid)??[]),c]);}
  children.set(null,[...(children.get(null)??[]),{id:"@trash",name:"Trash",article_ids:[],status:"active",deleted:true}]);
  for(const group of children.values())group.sort((a,b)=>a.name.localeCompare(b.name,undefined,{numeric:true}));
  const rows:Array<{collection:Collection;depth:number}>=[];const stack=(children.get(null)??[]).map(collection=>({collection,depth:1})).reverse();const seen=new Set<string>();
  while(stack.length){const row=stack.pop()!;if(seen.has(row.collection.id))continue;seen.add(row.collection.id);rows.push(row);if(expanded.has(row.collection.id))for(const child of [...(children.get(row.collection.id)??[])].reverse())stack.push({collection:child,depth:row.depth+1});}
  const excluded=new Set(moving?[moving]:[]);for(let i=0;i<collections.length;i++)for(const c of collections)if(c.parent_id && excluded.has(c.parent_id))excluded.add(c.id);
  const path=(id:string)=>{const names:string[]=[];const visited=new Set<string>();let c=collections.find(c=>c.id===id);while(c && !visited.has(c.id)){visited.add(c.id);names.unshift(c.name);c=collections.find(parent=>parent.id===c?.parent_id);}return names.join(" / ");};
  async function manage(id:string,action:"rename"|"move"|"restore"|"erase",value?:string|null){try{setActionError("");await onManage(id,action,value);}catch(e){setActionError(String(e));throw e;}}
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [deleteId, setDeleteId] = useState<string | null>(null);
  const pendingDelete = collections.find((collection) => collection.id === deleteId);
  const filedIds = new Set(collections.filter(c=>!c.deleted).flatMap((collection) => collection.article_ids));
  const unfiledCount = Math.max(0, allCount - filedIds.size);

  async function submitName(event: React.FormEvent) {
    event.preventDefault();
    if (!name.trim()) return;
    await onCreate(name.trim(),parentId);
    setName("");
    setCreating(false);
  }

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden gap-0">
      <CardHeader className="shrink-0">
        <div className="flex items-start justify-between gap-2">
          <div>
            <CardTitle>Collections</CardTitle>
            <CardDescription>{collections.length} collections</CardDescription>
          </div>
          <Button
            variant="outline"
            size="icon"
            aria-label="New collection"
            onClick={() => {setParentId(null);setCreating((current) => !current);}}
          >
            <Plus />
          </Button>
        </div>
      </CardHeader>

      {creating && (
        <CardContent className="shrink-0 pb-3">
          <form className="flex flex-wrap gap-2" onSubmit={event=>void submitName(event).catch(e=>setActionError(String(e)))}>
            <select aria-label="Parent collection" value={parentId??""} onChange={e=>setParentId(e.target.value||null)} className="w-full rounded border bg-background p-1 text-sm"><option value="">Library root</option>{collections.filter(c=>!c.deleted).map(c=><option key={c.id} value={c.id}>{path(c.id)}</option>)}</select>
            <Input
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Collection name"
            />
            <Button type="submit" size="sm" disabled={!name.trim()}>
              Save
            </Button>
          </form>
        </CardContent>
      )}

      {actionError && <p role="alert" className="p-2 text-sm text-destructive">{actionError}</p>}
      {moving && <form className="space-y-2 p-2" onSubmit={e=>{e.preventDefault();void manage(moving,"move",moveParent||null).then(()=>setMoving(null)).catch(()=>{});}}><label className="text-sm">Move to<select aria-label="Move destination" value={moveParent} onChange={e=>setMoveParent(e.target.value)} className="block w-full rounded border bg-background p-1"><option value="">Library root</option>{collections.filter(c=>!c.deleted && !excluded.has(c.id)).map(c=><option key={c.id} value={c.id}>{path(c.id)}</option>)}</select></label><Button type="submit" size="sm">Move</Button><Button type="button" size="sm" variant="ghost" onClick={()=>setMoving(null)}>Cancel</Button></form>}
      <label className="flex items-center gap-2 px-3 py-2 text-xs"><input type="checkbox" checked={recursive} onChange={e=>onRecursive(e.target.checked)}/>Show items from subcollections</label>
      <Separator className="shrink-0" />
      <ScrollArea className="min-h-0 flex-1">
        <CardContent className="p-2">
          <nav className="space-y-1" aria-label="Collections">
            <Button
              variant={activeView === "all" ? "secondary" : "ghost"}
              className="w-full justify-between px-2"
              onClick={() => onSelect("all")}
            >
              <span className="flex min-w-0 items-center gap-2">
                <Library className="size-4 text-primary" />
                <span className="truncate">All Records</span>
              </span>
              <Badge variant="outline">{allCount}</Badge>
            </Button>

            <Button
              variant={activeView === "unfiled" ? "secondary" : "ghost"}
              className="w-full justify-between px-2"
              onClick={() => onSelect("unfiled")}
            >
              <span className="flex min-w-0 items-center gap-2">
                <Inbox className="size-4 text-muted-foreground" />
                <span className="truncate">Unfiled</span>
              </span>
              <Badge variant="outline">{unfiledCount}</Badge>
            </Button>

            <Separator className="my-2" />

            <div ref={treeRef} role="tree" aria-label="Collection hierarchy">
            {rows.map(({collection,depth},rowIndex) => {
              if(collection.id==="@trash")return <button key="@trash" type="button" role="treeitem" aria-level={1} aria-expanded={expanded.has("@trash")} className="mt-2 flex w-full items-center gap-2 border-t p-2 text-sm" onClick={()=>toggle("@trash")}><Trash2 className="size-4"/>Trash</button>;
              const key = `col:${collection.id}` as const;
              return (
                <div
                  key={collection.id}
                  data-collection-id={collection.id}
                  tabIndex={(focused??(activeView.startsWith("col:")?activeView.slice(4):rows[0]?.collection.id))===collection.id?0:-1}
                  onFocus={()=>setFocused(collection.id)}
                  onKeyDown={event=>{
                    let target:string|undefined;
                    if(event.key==="ArrowDown")target=rows[rowIndex+1]?.collection.id;
                    if(event.key==="ArrowUp")target=rows[rowIndex-1]?.collection.id;
                    if(event.key==="Home")target=rows[0]?.collection.id;
                    if(event.key==="End")target=rows.at(-1)?.collection.id;
                    if(event.key==="ArrowRight"){if(!expanded.has(collection.id))toggle(collection.id);else target=rows[rowIndex+1]?.collection.id;}
                    if(event.key==="ArrowLeft"){if(expanded.has(collection.id))toggle(collection.id);else target=collection.parent_id??undefined;}
                    if(event.key==="Enter" && !collection.deleted)onSelect(key);
                    if(event.key.startsWith("Arrow") || ["Home","End"].includes(event.key))event.preventDefault();
                    if(target)focusRow(target);
                  }}
                  role="treeitem" aria-level={depth} aria-expanded={children.has(collection.id)?expanded.has(collection.id):undefined} aria-selected={activeView===key}
                  style={{marginLeft:(depth-1)*16}}
                  draggable={!collection.deleted}
                  onDragStart={e=>e.dataTransfer.setData("application/x-autonomics-collection",collection.id)}
                  onDragOver={e=>{if(!collection.deleted)e.preventDefault();}}
                  onDrop={e=>{e.preventDefault();const id=e.dataTransfer.getData("application/x-autonomics-collection");if(id)void manage(id,"move",collection.id).catch(()=>{});}}
                  className="group flex items-center rounded-md transition-colors hover:bg-accent"
                >
                  <button type="button" aria-label={`Toggle ${collection.name}`} className="w-5 shrink-0" onClick={()=>toggle(collection.id)}>{children.has(collection.id)?expanded.has(collection.id)?<ChevronDown className="size-4"/>:<ChevronRight className="size-4"/>:null}</button>
                  <Button
                    disabled={collection.deleted}
                    variant={activeView === key ? "secondary" : "ghost"}
                    className="flex min-h-10 min-w-0 flex-1 items-start justify-between gap-2 px-3 py-2 group-hover:bg-transparent"
                    onClick={() => onSelect(key)}
                  >
                    <span className="flex min-w-0 flex-1 items-start gap-2">
                      <FolderOpen className="size-4 text-primary" />
                      <span className="min-w-0 flex-1 whitespace-normal break-words text-left leading-tight">{collection.deleted?"[Trash] ":""}{collection.name}</span>
                    </span>
                    <Badge variant="outline">{collection.article_ids.length}</Badge>
                  </Button>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="size-8 opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                        aria-label={`Actions for ${collection.name}`}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {collection.deleted ? <>
                        <DropdownMenuItem onSelect={()=>void manage(collection.id,"restore").catch(()=>{})}>Restore collection</DropdownMenuItem>
                        <DropdownMenuItem onSelect={()=>setDeleteId(collection.id)}>Delete permanently…</DropdownMenuItem>
                      </> : <>
                        <DropdownMenuItem onSelect={()=>{setParentId(collection.id);setName("");setCreating(true);}}>New subcollection…</DropdownMenuItem>
                        <DropdownMenuItem onSelect={()=>{const name=window.prompt("Collection name",collection.name);if(name?.trim())void manage(collection.id,"rename",name.trim()).catch(()=>{});}}>Rename collection…</DropdownMenuItem>
                        <DropdownMenuItem onSelect={()=>{setMoving(collection.id);setMoveParent(collection.parent_id??"");}}>Move collection…</DropdownMenuItem>
                        <DropdownMenuItem onSelect={() => setDeleteId(collection.id)}><Trash2 /> Move to trash…</DropdownMenuItem>
                      </>}
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              );
            })}

            </div>
            {!collections.length && (
              <p className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">
                No collections
              </p>
            )}
          </nav>
        </CardContent>
      </ScrollArea>

      <AlertDialog open={Boolean(pendingDelete)} onOpenChange={(open) => !open && setDeleteId(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{pendingDelete?.deleted ? "Delete permanently?" : "Move collection to trash?"}</AlertDialogTitle>
            <AlertDialogDescription>
              {pendingDelete?.name} and its subcollections. Articles remain in your library.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingDelete) void (pendingDelete.deleted ? manage(pendingDelete.id,"erase") : onDelete(pendingDelete.id)).then(()=>setDeleteId(null)).catch(e=>setActionError(String(e)));
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}
