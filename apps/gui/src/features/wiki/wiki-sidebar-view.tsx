import { ViewLocationMenu } from "@/app/workbench/view-location-menu";
/**
 * Wiki sidebar: the browse view (formerly the "home" tab) plus the global
 * actions that used to live in the panel's tab row — new entry, search
 * (Ctrl+K) and the recycle bin. Entries open as workbench editor tabs via
 * the wiki opener bridge.
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { FileText, FilePlus2, Folder, Network, Loader2, RotateCcw, Search, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
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

export function WikiSidebarView({ onOpenGraph, onCloseGraph, graphActive, graph }: { onOpenGraph: () => void; onCloseGraph: () => void; graphActive: boolean; graph: ReactNode }) {
  const [entries, setEntries] = useState<EntrySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [creating, setCreating] = useState(false);
  const [recycleOpen, setRecycleOpen] = useState(false);
  const [recycle, setRecycle] = useState<RecycleItem[] | null>(null);
  const revision = useWikiRevision();
  const { t } = useTranslation("wiki");
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
        toast.success(t("sidebar.restored", { title: item.entry.title }));
      } else {
        await purgeEntry(item.entry.id);
        toast.success(t("sidebar.purged", { title: item.entry.title }));
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
        <Button size="icon-xs" variant={!graphActive && mode === "browse" ? "secondary" : "ghost"} aria-label={t("sidebar.browse")} title={t("sidebar.browse")} onClick={() => { onCloseGraph(); setMode("browse"); }}><Folder /></Button>
        <Button size="icon-xs" variant={!graphActive && mode === "search" ? "secondary" : "ghost"} aria-label={t("sidebar.searchEntries")} title={t("sidebar.searchEntries")} onClick={() => { onCloseGraph(); setMode("search"); requestAnimationFrame(() => inputRef.current?.focus()); }}><Search /></Button>
        <Button size="icon-xs" variant={graphActive ? "secondary" : "ghost"} aria-label={t("sidebar.graph")} title={t("sidebar.graph")} onClick={onOpenGraph}><Network /></Button>
        <span className="flex-1" />
        <Button size="icon-xs" variant="ghost" aria-label={t("sidebar.searchShortcut")} title={t("sidebar.searchShortcut")} onClick={openWikiSearch}><Search /></Button>
        <Button size="icon-xs" variant="ghost" aria-label={t("sidebar.newEntry")} title={t("sidebar.newEntry")} disabled={creating} onClick={() => void createNew()}>{creating ? <Loader2 className="animate-spin" /> : <FilePlus2 />}</Button>
        <Button size="icon-xs" variant="ghost" aria-label={t("sidebar.recycleBin")} title={t("sidebar.recycleBin")} onClick={() => void (async () => { setRecycleOpen(true); setRecycle(await listRecycle().catch(() => [])); })()}><Trash2 /></Button>
        <ViewLocationMenu id="wiki" primaryOnly />
      </div>
      {graphActive ? graph : <>
      {mode === "search" && <div className="shrink-0 px-3 pt-3">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input ref={inputRef} aria-label={t("sidebar.searchAria")} placeholder={t("sidebar.searchPlaceholder")} value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === "Escape") setQuery(""); }} className="h-8 pl-8 pr-8 text-sm" />
          {query && <button type="button" aria-label={t("sidebar.clearSearch")} className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground" onClick={() => { setQuery(""); inputRef.current?.focus(); }}><X className="size-4" /></button>}
        </div>
      </div>}
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-4 text-xs text-muted-foreground">
        <span>{searching ? t("sidebar.searching") : searchingQuery ? t("sidebar.resultsCount", { count: `${results.length}${results.length === 50 ? "+" : ""}` }) : t("sidebar.shownCount", { count: entries.length })}</span>
        <button type="button" className="shrink-0 hover:text-foreground" onClick={() => setSort(sort === "name" ? "recent" : "name")} aria-label={t("sidebar.toggleSort")}>{sort === "name" ? t("sidebar.sortName") : t("sidebar.sortRecent")} ↕</button>
      </div>
      <ScrollArea className="min-h-0 flex-1" contentClassName="px-2 py-2">
        {(loading && !searchingQuery) || (searching && !results.length) ? <div className="px-2 py-3 text-xs text-muted-foreground">{t("sidebar.loading")}</div> : shown.length === 0 ? <div className="px-2 py-3 text-xs text-muted-foreground">{searchingQuery ? t("common.noMatches") : t("common.noEntries")}</div> : (
          <div role="list" aria-label={searchingQuery ? t("sidebar.resultsAria") : t("sidebar.listAria")}>
            {shown.map((item) => <div key={item.id} role="listitem">
              <button type="button" title={item.title} className={cn("flex min-h-7 w-full items-start gap-1 rounded-sm px-1 py-1 text-left text-[13px] leading-5 text-muted-foreground hover:bg-accent hover:text-foreground")} onClick={() => openWikiEntry(item.id, { title: item.title })} onAuxClick={(event) => { if (event.button === 1) { event.preventDefault(); openWikiEntry(item.id, { title: item.title }); } }}>
                <FileText className="mt-0.5 size-4 shrink-0 opacity-50" /><span className="min-w-0 break-words">{item.title}</span>
              </button>
            </div>)}
          </div>
        )}
        {!searchingQuery && hasMore && !loading && <Button size="sm" variant="ghost" disabled={loadingMore} onClick={() => void loadBrowse(entries.length)}>{loadingMore && <Loader2 className="animate-spin" />}{t("common.loadMore", { count: entries.length })}</Button>}
      </ScrollArea>
      </>}
      <Dialog open={recycleOpen} onOpenChange={setRecycleOpen}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle>{t("sidebar.recycleTitle")}</DialogTitle>
            <DialogDescription>
              {t("sidebar.recycleDescription")}
            </DialogDescription>
          </DialogHeader>
          {recycle === null ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : recycle.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("sidebar.recycleEmpty")}</p>
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
                      {t("sidebar.deletedAt", { time: formatTime(item.deleted_at) })}
                    </span>
                  </span>
                  <span className="flex shrink-0 gap-1">
                    <Button size="sm" variant="outline" onClick={() => void recycleAction(item, "restore")}>
                      <RotateCcw className="h-3.5 w-3.5" /> {t("sidebar.restore")}
                    </Button>
                    <Button size="sm" variant="destructive" onClick={() => void recycleAction(item, "purge")}>
                      {t("sidebar.purge")}
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
