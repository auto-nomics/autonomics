import {
  BookOpen,
  Loader2,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { EntryEditor } from "@/components/wiki/entry-editor";
import { EntryList, type ListView } from "@/components/wiki/entry-list";
import { EntryReader } from "@/components/wiki/entry-reader";
import { SidePanel } from "@/components/wiki/side-panel";
import { Button } from "@/components/ui/button";
import { SimpleDialog } from "@/components/ui/simple-dialog";
import { Input } from "@/components/ui/input";
import {
  createEntry,
  deleteEntry,
  formatTime,
  getEntry,
  listEntries,
  listRecycle,
  purgeEntry,
  restoreEntry,
  searchEntries,
  type EntrySummary,
  type RecycleItem,
  type WikiEntry,
} from "@/lib/wiki-api";

/**
 * SiYuan-style three-pane wiki workspace:
 * left entry list (search + tag filter), center reader/editor, right
 * dockable side panel (backlinks / attachments / versions).
 */
export function WikiPanel() {
  const [entries, setEntries] = useState<EntrySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [view, setView] = useState<ListView>("browse");
  const [query, setQuery] = useState("");
  const [tagFilter, setTagFilter] = useState("");
  const [selected, setSelected] = useState<WikiEntry | null>(null);
  const [mode, setMode] = useState<"read" | "edit">("read");
  const [creating, setCreating] = useState(false);
  const [sideOpen, setSideOpen] = useState(true);
  const [recycleOpen, setRecycleOpen] = useState(false);
  const [recycle, setRecycle] = useState<RecycleItem[] | null>(null);

  const parseTags = (raw: string) =>
    raw
      .split(",")
      .map((tag) => tag.trim())
      .filter(Boolean);

  const loadBrowse = useCallback(
    async (tags: string) => {
      setLoading(true);
      try {
        setView("browse");
        setEntries(await listEntries({ tags: parseTags(tags), limit: 200 }));
      } catch (error) {
        toast.error((error as Error).message);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  const loadSearch = useCallback(async (term: string, tags: string) => {
    setLoading(true);
    try {
      setView("search");
      const hits = term.trim()
        ? await searchEntries(term, { limit: 50, tags: parseTags(tags) })
        : await listEntries({ tags: parseTags(tags), limit: 200 });
      // hydrate summaries for list rendering (titles/tags/mtime).
      const hydrated = await Promise.all(
        hits.map(async (hit) => {
          try {
            const entry = await getEntry(hit.entry_id);
            const { content: _content, ...summary } = entry;
            return summary;
          } catch {
            // Purged between search and hydration — drop the hit.
            return null;
          }
        }),
      );
      setEntries(hydrated.filter((item): item is EntrySummary => item !== null));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadBrowse("").catch((error: Error) => toast.error(error.message));
  }, [loadBrowse]);

  const openEntry = useCallback(async (id: string) => {
    try {
      setSelected(await getEntry(id));
      setMode("read");
    } catch (error) {
      toast.error((error as Error).message);
    }
  }, []);

  const refreshSelected = useCallback(async () => {
    if (!selected) return;
    try {
      setSelected(await getEntry(selected.id));
    } catch {
      // Deleted elsewhere — close the pane.
      setSelected(null);
    }
  }, [selected]);

  async function createNew() {
    setCreating(true);
    try {
      const base = `Untitled ${new Date().toLocaleString()}`;
      const created = await createEntry({
        title: base,
        content: `# ${base}\n\n`,
        tags: tagFilter ? parseTags(tagFilter) : [],
      });
      await openEntry(created.id);
      setMode("edit");
      await loadBrowse(tagFilter);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setCreating(false);
    }
  }

  async function softDelete() {
    if (!selected) return;
    await deleteEntry(selected.id);
    toast.success("Moved to recycle bin (title stays reserved until purge)");
    setSelected(null);
    await loadBrowse(tagFilter);
  }

  async function openRecycle() {
    setRecycleOpen(true);
    setRecycle(await listRecycle().catch(() => []));
  }

  async function recycleAction(item: RecycleItem, action: "restore" | "purge") {
    if (action === "restore") {
      await restoreEntry(item.entry.id);
      toast.success(`Restored "${item.entry.title}"`);
    } else {
      await purgeEntry(item.entry.id);
      toast.success(`Purged "${item.entry.title}"`);
    }
    setRecycle(await listRecycle().catch(() => []));
    await loadBrowse(tagFilter);
  }

  const words = selected ? selected.content.length : 0;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Top toolbar — SiYuan-style: brand, global search, actions, dock toggle */}
      <div className="flex items-center gap-2 border-b px-4 py-2">
        <span className="flex items-center gap-1.5 text-sm font-semibold">
          <BookOpen className="h-4 w-4" /> Wiki
        </span>
        <div className="ml-2 flex min-w-0 flex-1 items-center gap-2">
          <Input
            className="h-8 max-w-md"
            placeholder="Full-text search (BM25, 中英文)…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void loadSearch(query, tagFilter);
            }}
          />
          <Button size="sm" variant="ghost" onClick={() => void loadSearch(query, tagFilter)}>
            <Search className="h-4 w-4" />
          </Button>
          <Button size="sm" variant="outline" onClick={() => void loadBrowse(tagFilter)}>
            Browse all
          </Button>
        </div>
        <Button size="sm" variant="outline" onClick={() => void openRecycle()}>
          <Trash2 className="h-4 w-4" /> Recycle bin
        </Button>
        <Button size="sm" variant="outline" onClick={() => void createNew()} disabled={creating}>
          {creating ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />} New
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => setSideOpen((value) => !value)}
          title={sideOpen ? "Close side panel" : "Open side panel"}
        >
          {sideOpen ? <PanelRightClose className="h-4 w-4" /> : <PanelRightOpen className="h-4 w-4" />}
        </Button>
      </div>

      {/* Three panes */}
      <main className="grid min-h-0 flex-1 gap-3 p-3 lg:grid-cols-[280px_minmax(0,1fr)_320px] lg:gap-4">
        <section className="flex min-h-0 lg:h-full">
          <div className="w-full min-w-0">
            <EntryList
              entries={entries}
              loading={loading}
              view={view}
              query={query}
              setQuery={setQuery}
              tagFilter={tagFilter}
              setTagFilter={setTagFilter}
              onSearch={() => void loadSearch(query, tagFilter)}
              onBrowse={() => void loadBrowse(tagFilter)}
              selectedId={selected?.id}
              countLabel={String(entries.length)}
              onCreate={() => void createNew()}
              onSelect={(item) => {
                const id = "entry_id" in item ? item.entry_id : item.id;
                void openEntry(id);
              }}
            />
          </div>
        </section>

        <section className="flex min-h-0 flex-col lg:h-full">
          {selected ? (
            mode === "edit" ? (
              <div className="flex h-full min-h-0 flex-col">
                <ModeBar
                  mode={mode}
                  title={selected.title}
                  words={words}
                  mtime={selected.mtime}
                  onMode={setMode}
                />
                <div className="min-h-0 flex-1 pt-2">
                  <EntryEditor
                    entry={selected}
                    onCancel={() => setMode("read")}
                    onSaved={(entry) => {
                      setSelected(entry);
                      setMode("read");
                      void loadBrowse(tagFilter);
                    }}
                  />
                </div>
              </div>
            ) : (
              <div className="flex h-full min-h-0 flex-col">
                <ModeBar
                  mode={mode}
                  title={selected.title}
                  words={words}
                  mtime={selected.mtime}
                  onMode={setMode}
                />
                <div className="flex min-h-0 flex-1 flex-col pt-2">
                  <EntryReader entry={selected} />
                </div>
              </div>
            )
          ) : (
            <div className="flex h-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
              Select an entry, or create a new one.
            </div>
          )}
          {selected && mode === "read" ? (
            <div className="flex justify-end gap-2 pt-2">
              <Button size="sm" variant="outline" onClick={() => setMode("edit")}>
                Edit
              </Button>
              <Button size="sm" variant="outline" onClick={() => void softDelete()}>
                <Trash2 className="h-3.5 w-3.5" /> Delete
              </Button>
            </div>
          ) : null}
        </section>

        {sideOpen ? (
          <section className="hidden min-h-0 lg:flex lg:h-full lg:flex-col">
            <SidePanel entry={selected} />
          </section>
        ) : null}
      </main>

      {/* Recycle bin dialog — SiYuan "数据历史" analog */}
      <SimpleDialog
        open={recycleOpen}
        onClose={() => setRecycleOpen(false)}
        title="Recycle bin"
        description="Soft-deleted entries keep occupying their titles until restored or purged."
      >
        {recycle === null ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : recycle.length === 0 ? (
          <p className="text-sm text-muted-foreground">Recycle bin is empty.</p>
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
                    deleted {formatTime(item.deleted_at)}
                  </span>
                </span>
                <span className="flex shrink-0 gap-1">
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void recycleAction(item, "restore")}
                  >
                    <RotateCcw className="h-3.5 w-3.5" /> Restore
                  </Button>
                  <Button
                    size="sm"
                    variant="destructive"
                    onClick={() => void recycleAction(item, "purge")}
                  >
                    Purge
                  </Button>
                </span>
              </li>
            ))}
          </ul>
        )}
      </SimpleDialog>
    </div>
  );
}

/** Center-pane top strip: title, read/edit mode, and word/mtime status. */
function ModeBar({
  mode,
  title,
  words,
  mtime,
  onMode,
}: {
  mode: "read" | "edit";
  title: string;
  words: number;
  mtime: number;
  onMode: (mode: "read" | "edit") => void;
}) {
  return (
    <div className="flex items-center gap-2 rounded-md border px-3 py-1.5">
      <span className="min-w-0 flex-1 truncate text-sm font-medium">{title}</span>
      <span className="hidden text-xs text-muted-foreground xl:inline">
        {words} chars · modified {formatTime(mtime)}
      </span>
      <span className="flex overflow-hidden rounded-md border text-xs">
        {(["read", "edit"] as const).map((value) => (
          <button
            key={value}
            type="button"
            className={`px-2 py-1 capitalize transition-colors ${
              mode === value ? "bg-accent font-medium" : "hover:bg-accent"
            }`}
            onClick={() => onMode(value)}
          >
            {value}
          </button>
        ))}
      </span>
    </div>
  );
}
