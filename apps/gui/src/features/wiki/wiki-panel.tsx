/**
 * SiYuan-style wiki workspace: left entry list (BM25 search + tag
 * filter), center reader/editor, right dock (links / files / history),
 * recycle-bin dialog — all over gatewayRequest (cookie session).
 */
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Link2,
  Loader2,
  Pencil,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  Upload,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { renderMarkdown } from "@/lib/markdown";
import {
  attachmentRawUrl,
  createEntry,
  deleteEntry,
  detachAttachment,
  formatTime,
  getEntry,
  getLinks,
  getMentions,
  listAttachments,
  listEntries,
  listRecycle,
  listVersions,
  purgeEntry,
  putEntryContent,
  restoreEntry,
  restoreVersion,
  searchEntries,
  updateEntryMeta,
  uploadAttachment,
  type AttachmentMeta,
  type EntrySummary,
  type RecycleItem,
  type SearchHit,
  type VersionSummary,
  type WikiEntry,
} from "./wiki-api";

type Mode = "read" | "edit";
type ListRow = EntrySummary;

function rowId(row: ListRow): string {
  return row.id;
}

function rowTitle(row: ListRow): string {
  return row.title;
}

export function WikiPanel() {
  const [entries, setEntries] = useState<ListRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [tagFilter, setTagFilter] = useState("");
  const [selected, setSelected] = useState<WikiEntry | null>(null);
  const [mode, setMode] = useState<Mode>("read");
  const [creating, setCreating] = useState(false);
  const [dockOpen, setDockOpen] = useState(() => {
    try {
      return localStorage.getItem("wiki.dock.open") === "true";
    } catch {
      return false;
    }
  });
  const toggleDock = () =>
    setDockOpen((value) => {
      try {
        localStorage.setItem("wiki.dock.open", String(!value));
      } catch {
        // storage unavailable — per-session default only.
      }
      return !value;
    });
  const [recycleOpen, setRecycleOpen] = useState(false);
  const [recycle, setRecycle] = useState<RecycleItem[] | null>(null);

  const parseTags = (raw: string) =>
    raw.split(",").map((tag) => tag.trim()).filter(Boolean);

  const loadBrowse = useCallback(async (tags: string) => {
    setLoading(true);
    try {
      setEntries(await listEntries({ tags: parseTags(tags), limit: 200 }));
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  const loadSearch = useCallback(async (term: string, tags: string) => {
    setLoading(true);
    try {
      const hits = term.trim()
        ? await searchEntries(term, { limit: 50, tags: parseTags(tags) })
        : await listEntries({ tags: parseTags(tags), limit: 200 });
      const hydrated = await Promise.all(
        (hits as SearchHit[]).map(async (hit) => {
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
  // Side-panel link clicks open the entry in the center pane.
  useEffect(() => {
    const handler = (event: Event) => {
      const id = (event as CustomEvent<string>).detail;
      if (typeof id === "string") void openEntry(id);
    };
    window.addEventListener("wiki:open-entry", handler);
    return () => window.removeEventListener("wiki:open-entry", handler);
  }, [openEntry]);

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
    toast.success("已移入回收站(标题在净删前仍被占用)");
    setSelected(null);
    await loadBrowse(tagFilter);
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
      await loadBrowse(tagFilter);
    } catch (error) {
      toast.error((error as Error).message);
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <span className="flex items-center gap-1.5 text-sm font-semibold">
          <BookOpen className="h-4 w-4" /> 知识库
        </span>
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <Input
            className="h-8 max-w-md"
            placeholder="全文检索(BM25,中英文)…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void loadSearch(query, tagFilter);
            }}
          />
          <Button size="sm" variant="ghost" onClick={() => void loadSearch(query, tagFilter)}>
            <Search className="h-4 w-4" />
          </Button>
          <Input
            className="h-8 w-44"
            placeholder="标签过滤(逗号分隔,AND)"
            value={tagFilter}
            onChange={(event) => setTagFilter(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void loadBrowse(tagFilter);
            }}
          />
          <Button size="sm" variant="outline" onClick={() => void loadBrowse(tagFilter)}>
            浏览全部
          </Button>
        </div>
        <Button size="sm" variant="outline" onClick={() => void (async () => {
          setRecycleOpen(true);
          setRecycle(await listRecycle().catch(() => []));
        })()}>
          <Trash2 className="h-4 w-4" /> 回收站
        </Button>
        <Button size="sm" variant="outline" onClick={() => void createNew()} disabled={creating}>
          {creating ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />} 新建
        </Button>
      </div>

      <main className="grid min-h-0 flex-1 gap-3 overflow-auto p-3 lg:grid-cols-[280px_minmax(0,1fr)] lg:gap-4 lg:overflow-hidden">
        <section className="flex min-h-0 flex-col lg:h-full">
          <Card className="flex h-full min-h-0 flex-col">
            <CardHeader className="pb-2">
              <span className="text-xs text-muted-foreground">{entries.length} 条词条</span>
            </CardHeader>
            <CardContent className="min-h-0 flex-1">
              <ScrollArea className="h-full pr-2">
                {loading ? (
                  <div className="space-y-2">
                    {Array.from({ length: 6 }).map((_, index) => (
                      <Skeleton key={index} className="h-12 w-full" />
                    ))}
                  </div>
                ) : entries.length === 0 ? (
                  <p className="py-8 text-center text-sm text-muted-foreground">没有匹配的词条。</p>
                ) : (
                  <ul className="space-y-1">
                    {entries.map((item) => {
                      const id = rowId(item);
                      const summary = item as EntrySummary;
                      const active = selected?.id === id;
                      return (
                        <li key={id}>
                          <button
                            type="button"
                            className={`w-full rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent ${active ? "bg-accent" : ""}`}
                            onClick={() => void openEntry(id)}
                          >
                            <div className="truncate text-sm font-medium">{rowTitle(item)}</div>
                            <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                              <span>{formatTime(summary.mtime)}</span>
                              {(summary.tags ?? []).slice(0, 3).map((tag) => (
                                <Badge key={tag} variant="secondary" className="px-1 py-0 text-[10px]">
                                  {tag}
                                </Badge>
                              ))}
                            </div>
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </ScrollArea>
            </CardContent>
          </Card>
        </section>

        <section className="flex min-h-0 flex-col lg:h-full">
          {selected === null ? (
            <div className="flex h-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
              选择左侧词条,或新建一个。
            </div>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col gap-2">
              {mode === "edit" ? (
                <div className="flex min-h-0 flex-1 flex-col">
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
              ) : (
                <>
                  <div className="flex min-h-0 flex-1 flex-col">
                    <EntryReader
                      entry={selected}
                      onEdit={() => setMode("edit")}
                      onDelete={() => void softDelete()}
                    />
                  </div>
                  {/* 折叠区块:链接 / 附件 / 历史(原右侧栏内容) */}
                  <div className="rounded-md border">
                    <button
                      type="button"
                      className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent"
                      onClick={toggleDock}
                    >
                      {dockOpen ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                      链接与历史
                    </button>
                    {dockOpen ? (
                      <div className="h-64 overflow-hidden border-t px-1 pb-1">
                        <SidePanel entry={selected} />
                      </div>
                    ) : null}
                  </div>
                </>
              )}
            </div>
          )}
        </section>
      </main>

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

function EntryReader({
  entry,
  onEdit,
  onDelete,
}: {
  entry: WikiEntry;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="pb-2">
        <div className="flex items-start justify-between gap-2">
          <h1 className="min-w-0 break-words text-lg font-semibold">{entry.title}</h1>
          <span className="flex shrink-0 gap-1">
            <Button size="sm" variant="outline" onClick={onEdit}>
              <Pencil className="h-3.5 w-3.5" /> 编辑
            </Button>
            <Button size="sm" variant="outline" onClick={() => void onDelete()}>
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          {entry.tags.map((tag) => (
            <Badge key={tag} variant="secondary">{tag}</Badge>
          ))}
          {entry.alias.map((name) => (
            <Badge key={name} variant="outline">alias: {name}</Badge>
          ))}
          <span className="ml-auto">
            创建 {formatTime(entry.ctime)} · 修改 {formatTime(entry.mtime)}
          </span>
        </div>
      </CardHeader>
      <CardContent className="min-h-0 flex-1">
        <ScrollArea className="h-full pr-3">
          {/* Server-sanitised on display via renderMarkdown (marked + DOMPurify). */}
          <div
            className="md prose prose-sm dark:prose-invert max-w-none break-words text-sm"
            dangerouslySetInnerHTML={{ __html: renderMarkdown(entry.content) }}
          />
        </ScrollArea>
      </CardContent>
    </Card>
  );
}

function EntryEditor({
  entry,
  onCancel,
  onSaved,
}: {
  entry: WikiEntry;
  onCancel: () => void;
  onSaved: (entry: WikiEntry) => void;
}) {
  const [title, setTitle] = useState(entry.title);
  const [alias, setAlias] = useState(entry.alias.join(", "));
  const [tags, setTags] = useState(entry.tags.join(", "));
  const [content, setContent] = useState(entry.content);
  const [saving, setSaving] = useState(false);

  const split = (value: string) =>
    value.split(",").map((item) => item.trim()).filter(Boolean);

  async function save() {
    setSaving(true);
    try {
      const aliasList = split(alias);
      const tagList = split(tags);
      const metaChanged =
        title !== entry.title ||
        aliasList.join(",") !== entry.alias.join(",") ||
        tagList.join(",") !== entry.tags.join(",");
      if (metaChanged) {
        const updated = await updateEntryMeta(entry.id, {
          new_title: title !== entry.title ? title : undefined,
          alias: aliasList,
          tags: tagList,
        });
        onSaved(updated);
      }
      if (content !== entry.content) {
        await putEntryContent(entry.id, content);
      }
      onSaved({ ...entry, title, alias: aliasList, tags: tagList, content });
      toast.success("已保存(新版本快照已追加)");
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="grid gap-2 pb-2">
        <div className="grid gap-1">
          <span className="text-xs text-muted-foreground">
            标题(唯一,自动 trim)
          </span>
          <Input id="wiki-title" value={title} onChange={(event) => setTitle(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <span className="text-xs text-muted-foreground">
            别名(逗号分隔)
          </span>
          <Input id="wiki-alias" value={alias} onChange={(event) => setAlias(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <span className="text-xs text-muted-foreground">
            标签(逗号分隔)
          </span>
          <Input id="wiki-tags" value={tags} onChange={(event) => setTags(event.target.value)} />
        </div>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col gap-2">
        <Textarea
          className="min-h-0 flex-1 font-mono text-sm"
          value={content}
          onChange={(event) => setContent(event.target.value)}
          spellCheck={false}
        />
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onCancel} disabled={saving}>
            取消
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            保存
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function SidePanel({ entry }: { entry: WikiEntry | null }) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      {entry === null ? (
        <div className="flex h-full items-center justify-center rounded-md border border-dashed text-xs text-muted-foreground">
          未选中词条。
        </div>
      ) : (
        <Tabs defaultValue="links" className="flex h-full min-h-0 flex-col gap-2">
          <TabsList className="grid w-full grid-cols-3">
            <TabsTrigger value="links">链接</TabsTrigger>
            <TabsTrigger value="files">附件</TabsTrigger>
            <TabsTrigger value="history">历史</TabsTrigger>
          </TabsList>
          <TabsContent value="links" className="min-h-0 flex-1 overflow-y-auto">
            <LinksTab entry={entry} />
          </TabsContent>
          <TabsContent value="files" className="min-h-0 flex-1 overflow-y-auto">
            <FilesTab entry={entry} />
          </TabsContent>
          <TabsContent value="history" className="min-h-0 flex-1 overflow-y-auto">
            <HistoryTab entry={entry} />
          </TabsContent>
        </Tabs>
      )}
    </div>
  );
}

function LinksTab({ entry }: { entry: WikiEntry }) {
  const [graph, setGraph] = useState<Awaited<ReturnType<typeof getLinks>> | null>(null);
  const [mentions, setMentions] = useState<string[] | null>(null);

  useEffect(() => {
    setGraph(null);
    setMentions(null);
    void getLinks(entry.id).then(setGraph).catch((error: Error) => toast.error(error.message));
    void getMentions(entry.id).then(setMentions).catch(() => setMentions([]));
  }, [entry.id]);

  if (graph === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  return (
    <div className="space-y-3 text-xs">
      <LinkSection title={`出链(${graph.out_links.length})`} ids={graph.out_links} onOpen={openEntryById} />
      <LinkSection title={`入链(${graph.in_links.length})`} ids={graph.in_links} onOpen={openEntryById} />
      <LinkSection
        title={`未链接提及(${mentions?.length ?? 0})`}
        ids={mentions ?? []}
        onOpen={openEntryById}
        empty={mentions !== null ? "无 —— 在其他词条中添加 [[链接]] 后这里会出现候选。" : undefined}
        loading={mentions === null}
      />
    </div>
  );
}

/** 侧栏点击 → 通知面板打开词条(与主列表共用一条事件通道)。 */
function openEntryById(id: string) {
  window.dispatchEvent(new CustomEvent("wiki:open-entry", { detail: id }));
}

function LinkSection({
  title,
  ids,
  onOpen,
  empty,
  loading,
}: {
  title: string;
  ids: string[];
  onOpen: (id: string) => void;
  empty?: string;
  loading?: boolean;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div>
      <button
        type="button"
        className="flex w-full items-center gap-1 py-1 font-medium"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        {title}
      </button>
      {open ? (
        <div className="space-y-0.5 pl-2">
          {loading ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : ids.length === 0 ? (
            <p className="text-muted-foreground">{empty ?? "无"}</p>
          ) : (
            ids.map((id) => (
              <button
                key={id}
                type="button"
                className="flex w-full items-center gap-1.5 truncate rounded px-1.5 py-1 text-left hover:bg-accent"
                title={id}
                onClick={() => onOpen(id)}
              >
                <Link2 className="h-3 w-3 shrink-0 text-muted-foreground" />
                <span className="truncate font-mono text-[11px]">{id}</span>
              </button>
            ))
          )}
        </div>
      ) : null}
    </div>
  );
}

function FilesTab({ entry }: { entry: WikiEntry }) {
  const [items, setItems] = useState<AttachmentMeta[] | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  async function reload() {
    setItems(await listAttachments(entry.id).catch(() => []));
  }

  useEffect(() => {
    setItems(null);
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entry.id]);

  return (
    <div className="space-y-2 text-xs">
      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (!file) return;
          void (async () => {
            try {
              await uploadAttachment(entry.id, file);
              toast.success(`已附加 ${file.name}`);
              await reload();
            } catch (error) {
              toast.error((error as Error).message);
            }
          })();
        }}
      />
      <Button size="sm" variant="outline" className="w-full" onClick={() => fileInput.current?.click()}>
        <Upload className="h-3.5 w-3.5" /> 上传图像
      </Button>
      {items === null ? (
        <Loader2 className="h-4 w-4 animate-spin" />
      ) : items.length === 0 ? (
        <p className="text-muted-foreground">暂无附件。</p>
      ) : (
        <ul className="grid grid-cols-2 gap-2">
          {items.map((item) => {
            const id = item.attachment_id ?? item.id ?? "";
            return (
              <li key={id} className="rounded-md border p-1.5">
                <a href={attachmentRawUrl(entry.id, id)} target="_blank" rel="noreferrer">
                  <img
                    src={attachmentRawUrl(entry.id, id)}
                    alt={item.filename}
                    className="h-20 w-full rounded object-contain"
                    loading="lazy"
                  />
                </a>
                <div className="mt-1 flex items-center justify-between gap-1">
                  <span className="truncate text-[11px]">{item.filename}</span>
                  <button
                    type="button"
                    className="rounded p-0.5 hover:bg-accent"
                    title="摘除"
                    onClick={() =>
                      void (async () => {
                        try {
                          await detachAttachment(entry.id, id);
                          await reload();
                        } catch (error) {
                          toast.error((error as Error).message);
                        }
                      })()
                    }
                  >
                    ✕
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function HistoryTab({ entry }: { entry: WikiEntry }) {
  const [versions, setVersions] = useState<VersionSummary[] | null>(null);

  useEffect(() => {
    setVersions(null);
    void listVersions(entry.id).then(setVersions).catch(() => setVersions([]));
  }, [entry.id]);

  if (versions === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  if (!versions.length) {
    return <p className="text-xs text-muted-foreground">暂无版本记录。</p>;
  }
  return (
    <ul className="space-y-1 text-xs">
      {versions.map((version) => (
        <li key={version.version} className="flex items-center gap-2 rounded px-1 py-1 hover:bg-accent">
          <span className="font-mono">v{version.version}</span>
          <span className="flex-1 text-muted-foreground">{formatTime(version.created_at)}</span>
          <Button
            size="sm"
            variant="ghost"
            className="h-6 px-2"
            onClick={() =>
              void (async () => {
                try {
                  await restoreVersion(entry.id, version.version);
                  toast.success(`已回滚到 v${version.version}(新版本已追加)`);
                } catch (error) {
                  toast.error((error as Error).message);
                }
              })()
            }
          >
            回滚
          </Button>
        </li>
      ))}
    </ul>
  );
}
