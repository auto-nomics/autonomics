/**
 * Wiki sidebar: the browse view (formerly the "home" tab) plus the global
 * actions that used to live in the panel's tab row — new entry, search
 * (Ctrl+K) and the recycle bin. Entries open as workbench editor tabs via
 * the wiki opener bridge.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { FileText, FilePlus2, Folder, Network, Loader2, RotateCcw, Search, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { PAGE_SIZE } from "./home-view";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  createEntry,
  formatTime,
  getEntry,
  searchEntries,
  listEntries,
  listRecycle,
  purgeEntry,
  restoreEntry,
  type RecycleItem,
  type EntrySummary,
} from "./wiki-api";
import {
  bumpWikiRevision,
  openWikiEntry,
  openWikiSearch,
  useWikiRevision,
} from "./wiki-state";

export function WikiSidebarView({ onOpenGraph }: { onOpenGraph: () => void }) {
  const [entries, setEntries] = useState<EntrySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [creating, setCreating] = useState(false);
  const [recycleOpen, setRecycleOpen] = useState(false);
  const [recycle, setRecycle] = useState<RecycleItem[] | null>(null);
  const revision = useWikiRevision();
  const [mode, setMode] = useState<"browse" | "search">("search");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<EntrySummary[]>([]);
  const [searching, setSearching] = useState(false);
  const [sort, setSort] = useState<"name" | "recent">("name");
  const inputRef = useRef<HTMLInputElement>(null);
  const searchVersion = useRef(0);

  useEffect(() => {
    const version = ++searchVersion.current;
    const term = query.trim();
    if (mode !== "search" || !term) { setResults([]); setSearching(false); return; }
    setResults([]);
    setSearching(true);
    const timer = window.setTimeout(() => {
      void searchEntries(term, { limit: 50 }).then(async (hits) => {
        const loaded = await Promise.all(hits.map(async (hit) => {
          try { return await getEntry(hit.entry_id); } catch { return null; }
        }));
        if (version === searchVersion.current) setResults(loaded.filter((item): item is NonNullable<typeof item> => item !== null));
      }).catch((error: Error) => {
        if (version === searchVersion.current) { setResults([]); toast.error(error.message); }
      }).finally(() => { if (version === searchVersion.current) setSearching(false); });
    }, 250);
    return () => { window.clearTimeout(timer); searchVersion.current++; };
  }, [mode, query, revision]);

  const searchingQuery = mode === "search" && Boolean(query.trim());
  const shown = [...(searchingQuery ? results : entries)].sort((a, b) => sort === "name" ? a.title.localeCompare(b.title, undefined, { numeric: true }) : b.mtime - a.mtime);

  const loadBrowse = useCallback(async (offset = 0) => {
    if (offset === 0) setLoading(true);
    else setLoadingMore(true);
    try {
      const page = await listEntries({ limit: PAGE_SIZE, offset });
      setEntries((prev) => (offset > 0 ? [...prev, ...page] : page));
      setHasMore(page.length === PAGE_SIZE);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, []);

  useEffect(() => {
    void loadBrowse().catch((error: Error) => toast.error(error.message));
  }, [loadBrowse, revision]);

  async function createNew() {
    setCreating(true);
    try {
      const base = `Untitled ${new Date().toLocaleString()}`;
      const created = await createEntry({
        title: base,
        content: `# ${base}\n\n`,
        tags: [],
      });
      bumpWikiRevision();
      openWikiEntry(created.id, { title: base, mode: "edit" });
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setCreating(false);
    }
  }

  async function recycleAction(item: RecycleItem, action: "restore" | "purge") {
    try {
      if (action === "restore") {
        await restoreEntry(item.entry.id);
        toast.success(`已恢复「${item.entry.title}」`);
      } else {
        await purgeEntry(item.entry.id);
        toast.success(`已净删「${item.entry.title}」`);
      }
      setRecycle(await listRecycle().catch(() => []));
      bumpWikiRevision();
    } catch (error) {
      toast.error((error as Error).message);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-0.5 border-b px-2">
        <Button size="icon-sm" variant={mode === "browse" ? "secondary" : "ghost"} aria-label="浏览词条" title="浏览词条" onClick={() => setMode("browse")}><Folder /></Button>
        <Button size="icon-sm" variant={mode === "search" ? "secondary" : "ghost"} aria-label="搜索词条" title="搜索词条" onClick={() => { setMode("search"); requestAnimationFrame(() => inputRef.current?.focus()); }}><Search /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="打开知识图谱" title="打开知识图谱" onClick={onOpenGraph}><Network /></Button>
        <span className="flex-1" />
        <Button size="icon-sm" variant="ghost" aria-label="检索(Ctrl+K)" title="检索(Ctrl+K)" onClick={openWikiSearch}><Search /></Button>
        <Button size="icon-sm" variant="ghost" aria-label="新建词条" title="新建词条" disabled={creating} onClick={() => void createNew()}>{creating ? <Loader2 className="animate-spin" /> : <FilePlus2 />}</Button>
        <Button size="icon-sm" variant="ghost" aria-label="回收站" title="回收站" onClick={() => void (async () => { setRecycleOpen(true); setRecycle(await listRecycle().catch(() => [])); })()}><Trash2 /></Button>
      </div>
      {mode === "search" && <div className="shrink-0 px-3 pt-3">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input ref={inputRef} aria-label="搜索知识库" placeholder="搜索知识库" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") setQuery(""); }} className="h-8 pl-8 pr-8 text-sm" />
          {query && <button type="button" aria-label="清除搜索" className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground" onClick={() => { setQuery(""); inputRef.current?.focus(); }}><X className="size-4" /></button>}
        </div>
      </div>}
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-4 text-xs text-muted-foreground">
        <span>{searching ? "搜索中…" : searchingQuery ? `${results.length}${results.length === 50 ? "+" : ""} 条结果` : `已显示 ${entries.length} 条词条`}</span>
        <button type="button" className="shrink-0 hover:text-foreground" onClick={() => setSort(sort === "name" ? "recent" : "name")} aria-label="切换词条排序">{sort === "name" ? "名称 (A to Z)" : "最近更新"} ↕</button>
      </div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
        {(loading && !searchingQuery) || (searching && !results.length) ? <div className="px-2 py-3 text-xs text-muted-foreground">加载中…</div> : shown.length === 0 ? <div className="px-2 py-3 text-xs text-muted-foreground">{searchingQuery ? "没有匹配的词条。" : "还没有词条。"}</div> : (
          <div role="list" aria-label={searchingQuery ? "检索结果" : "词条列表"}>
            {shown.map((item) => <div key={item.id} role="listitem">
              <button type="button" title={item.title} className={cn("flex min-h-7 w-full items-start gap-1 rounded-sm px-1 py-1 text-left text-[13px] leading-5 text-muted-foreground hover:bg-accent hover:text-foreground")} onClick={() => openWikiEntry(item.id, { title: item.title })} onAuxClick={(event) => { if (event.button === 1) { event.preventDefault(); openWikiEntry(item.id, { title: item.title }); } }}>
                <FileText className="mt-0.5 size-4 shrink-0 opacity-50" /><span className="min-w-0 break-words">{item.title}</span>
              </button>
            </div>)}
          </div>
        )}
        {!searchingQuery && hasMore && !loading && <Button size="sm" variant="ghost" disabled={loadingMore} onClick={() => void loadBrowse(entries.length)}>{loadingMore && <Loader2 className="animate-spin" />}加载更多(已显示 {entries.length} 条)</Button>}
      </ScrollArea>
      <Dialog open={recycleOpen} onOpenChange={setRecycleOpen}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle>回收站</DialogTitle>
            <DialogDescription>
              软删除的词条在恢复或净删前持续占用其标题。
            </DialogDescription>
          </DialogHeader>
          {recycle === null ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : recycle.length === 0 ? (
            <p className="text-sm text-muted-foreground">回收站为空。</p>
          ) : (
            <ul className="max-h-80 space-y-2 overflow-y-auto">
              {recycle.map((item) => (
                <li
                  key={item.entry.id}
                  className="flex items-center justify-between gap-2 rounded-md border p-2"
                >
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-medium">{item.entry.title}</span>
                    <span className="text-xs text-muted-foreground">
                      删除于 {formatTime(item.deleted_at)}
                    </span>
                  </span>
                  <span className="flex shrink-0 gap-1">
                    <Button size="sm" variant="outline" onClick={() => void recycleAction(item, "restore")}>
                      <RotateCcw className="h-3.5 w-3.5" /> 恢复
                    </Button>
                    <Button size="sm" variant="destructive" onClick={() => void recycleAction(item, "purge")}>
                      净删
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
