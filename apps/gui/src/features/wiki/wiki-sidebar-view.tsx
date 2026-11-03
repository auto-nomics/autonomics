/**
 * Wiki sidebar: the browse view (formerly the "home" tab) plus the global
 * actions that used to live in the panel's tab row — new entry, search
 * (Ctrl+K) and the recycle bin. Entries open as workbench editor tabs via
 * the wiki opener bridge.
 */

import { useCallback, useEffect, useState } from "react";
import { FilePlus2, Loader2, RotateCcw, Search, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { HomeView, PAGE_SIZE } from "./home-view";
import {
  createEntry,
  formatTime,
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

export function WikiSidebarView() {
  const [entries, setEntries] = useState<EntrySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [creating, setCreating] = useState(false);
  const [recycleOpen, setRecycleOpen] = useState(false);
  const [recycle, setRecycle] = useState<RecycleItem[] | null>(null);
  const revision = useWikiRevision();

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
      <div className="flex shrink-0 items-center gap-0.5 border-b px-1.5 py-1">
        <Button size="icon-sm" variant="ghost" title="检索(Ctrl+K)" onClick={openWikiSearch}>
          <Search />
        </Button>
        <Button size="icon-sm" variant="ghost" title="新建词条" disabled={creating} onClick={() => void createNew()}>
          {creating ? <Loader2 className="animate-spin" /> : <FilePlus2 />}
        </Button>
        <Button
          size="icon-sm"
          variant="ghost"
          title="回收站"
          onClick={() => void (async () => {
            setRecycleOpen(true);
            setRecycle(await listRecycle().catch(() => []));
          })()}
        >
          <Trash2 />
        </Button>
      </div>
      <HomeView
        entries={entries}
        loading={loading}
        hasMore={hasMore}
        loadingMore={loadingMore}
        onLoadMore={() => void loadBrowse(entries.length)}
        onOpen={(id, title) => openWikiEntry(id, { title })}
      />
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
