/**
 * SiYuan 式 wiki 工作台:浏览器式多标签页 —— 「首页」tab 承载 BM25 检索
 * 与分页浏览(home-view),每篇词条一个 tab;tab 内维护前进/后退历史栈
 * (点 [[双链]] 压栈,Ctrl/中键开新 tab),标签页与游标持久化到
 * localStorage。底部折叠 dock(链接 / 附件 / 历史)跟随当前词条 tab。
 */
import {
  ArrowLeft,
  ArrowRight,
  BookOpen,
  ChevronDown,
  ChevronRight,
  FileText,
  Link2,
  Loader2,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EditorView } from "./editor-view";
import { HomeView, PAGE_SIZE } from "./home-view";
import { ReadingView, type WikiMode } from "./reading-view";
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
  restoreEntry,
  restoreVersion,
  searchEntries,
  uploadAttachment,
  type AttachmentMeta,
  type EntrySummary,
  type RecycleItem,
  type SearchHit,
  type VersionSummary,
  type WikiEntry,
  type WikiLinkRef,
} from "./wiki-api";

type Mode = WikiMode;

/** tab 内的一个"位置":首页或某篇词条(带标题快照,取到后回填)。 */
type TabLocation = { kind: "home" } | { kind: "entry"; entryId: string; title: string };

interface WikiTab {
  id: string;
  /** 历史栈 + 游标:双链跳转截断游标之后并压栈,后退/前进只挪游标。 */
  stack: TabLocation[];
  cursor: number;
  mode: Mode;
}

const TABS_STORAGE_KEY = "wiki.tabs.v1";

function freshHomeTab(): WikiTab {
  return {
    id: crypto.randomUUID(),
    stack: [{ kind: "home" }],
    cursor: 0,
    mode: "read",
  };
}

/** 恢复上次会话的标签页;形状不对就当没有。 */
function loadPersistedTabs(): { tabs: WikiTab[]; activeId: string } | null {
  try {
    const raw = localStorage.getItem(TABS_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { tabs?: unknown; activeId?: unknown };
    if (!Array.isArray(parsed.tabs)) return null;
    const tabs = parsed.tabs.filter(
      (tab): tab is WikiTab =>
        typeof tab?.id === "string" &&
        Array.isArray(tab.stack) &&
        tab.stack.length > 0 &&
        typeof tab.cursor === "number" &&
        tab.cursor >= 0 &&
        tab.cursor < tab.stack.length,
    );
    if (tabs.length === 0) return null;
    const activeId =
      typeof parsed.activeId === "string" && tabs.some((tab) => tab.id === parsed.activeId)
        ? parsed.activeId
        : tabs[0].id;
    return { tabs, activeId };
  } catch {
    return null;
  }
}

export function WikiPanel() {
  const [boot] = useState(() => {
    const persisted = loadPersistedTabs();
    if (persisted) return persisted;
    const tab = freshHomeTab();
    return { tabs: [tab], activeId: tab.id };
  });
  const [tabs, setTabs] = useState<WikiTab[]>(boot.tabs);
  const [activeTabId, setActiveTabId] = useState<string>(boot.activeId);
  const entryCache = useRef(new Map<string, WikiEntry>());

  // —— 首页浏览状态(跨 tab 切换保留)——
  const [entries, setEntries] = useState<EntrySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [query, setQuery] = useState("");
  const [tagFilter, setTagFilter] = useState("");

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
  // 点击阅读器里不存在的 [[双链]] 时,给出一键创建(思源的断链流)。
  const [missingLink, setMissingLink] = useState<string | null>(null);
  const [creatingLink, setCreatingLink] = useState(false);
  const [creating, setCreating] = useState(false);

  const parseTags = (raw: string) =>
    raw.split(",").map((tag) => tag.trim()).filter(Boolean);

  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? tabs[0];
  const current = activeTab.stack[activeTab.cursor];
  const canBack = activeTab.cursor > 0;
  const canForward = activeTab.cursor < activeTab.stack.length - 1;

  // 标签页与游标持久化(标题是快照,失效无害——取到词条后回填)。
  useEffect(() => {
    try {
      localStorage.setItem(TABS_STORAGE_KEY, JSON.stringify({ tabs, activeId: activeTabId }));
    } catch {
      // storage unavailable — tabs live for the session only.
    }
  }, [tabs, activeTabId]);

  // —— 首页:浏览 / 检索 / 续拉 ——
  const loadBrowse = useCallback(async (tags: string, offset = 0) => {
    if (offset === 0) setLoading(true);
    else setLoadingMore(true);
    try {
      const page = await listEntries({ tags: parseTags(tags), limit: PAGE_SIZE, offset });
      setEntries((prev) => (offset > 0 ? [...prev, ...page] : page));
      setHasMore(page.length === PAGE_SIZE);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, []);

  const loadSearch = useCallback(async (term: string, tags: string) => {
    setLoading(true);
    try {
      const hits = term.trim()
        ? await searchEntries(term, { limit: 50, tags: parseTags(tags) })
        : await listEntries({ tags: parseTags(tags), limit: PAGE_SIZE });
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
      setHasMore(false);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadBrowse("").catch((error: Error) => toast.error(error.message));
  }, [loadBrowse]);

  // —— 词条缓存与 tab 标题回填 ——
  const applyEntry = useCallback((entry: WikiEntry) => {
    entryCache.current.set(entry.id, entry);
    setTabs((prev) =>
      prev.map((tab) => ({
        ...tab,
        stack: tab.stack.map((loc) =>
          loc.kind === "entry" && loc.entryId === entry.id
            ? { ...loc, title: entry.title }
            : loc,
        ),
      })),
    );
  }, []);

  const fetchEntry = useCallback(
    async (id: string): Promise<WikiEntry | null> => {
      const cached = entryCache.current.get(id);
      if (cached) return cached;
      try {
        const entry = await getEntry(id);
        applyEntry(entry);
        return entry;
      } catch (error) {
        toast.error((error as Error).message);
        return null;
      }
    },
    [applyEntry],
  );

  // —— 导航:压栈 / 后退 / 前进 / tab 管理 ——
  const guardEdit = useCallback((tab: WikiTab) => {
    if (tab.mode === "edit" && tab.stack[tab.cursor].kind === "entry") {
      return window.confirm("该标签页正在编辑中,离开将丢弃未保存的修改。继续?");
    }
    return true;
  }, []);

  /** 当前 tab 压栈跳转(截断前进分支);home tab 永远不被词条占据。 */
  const navigateActive = useCallback(
    (loc: TabLocation) => {
      if (!guardEdit(activeTab)) return;
      setTabs((prev) =>
        prev.map((tab) => {
          if (tab.id !== activeTab.id) return tab;
          const stack = [...tab.stack.slice(0, tab.cursor + 1), loc];
          return { ...tab, stack, cursor: stack.length - 1, mode: "read" };
        }),
      );
    },
    [activeTab, guardEdit],
  );

  /** 打开词条:显式新 tab,或当前 tab 是 home 时新开(home 保持浏览位)。 */
  const openEntry = useCallback(
    (entryId: string, title: string, newTab: boolean) => {
      const loc: TabLocation = { kind: "entry", entryId, title: title || "…" };
      if (newTab || current.kind === "home") {
        const id = crypto.randomUUID();
        setTabs((prev) => [...prev, { id, stack: [loc], cursor: 0, mode: "read" }]);
        setActiveTabId(id);
      } else {
        navigateActive(loc);
      }
      // 标题快照可能为空 — 后台取一次回填。
      void fetchEntry(entryId);
    },
    [current.kind, fetchEntry, navigateActive],
  );

  const stepHistory = useCallback(
    (delta: -1 | 1) => {
      const next = activeTab.cursor + delta;
      if (next < 0 || next >= activeTab.stack.length) return;
      if (!guardEdit(activeTab)) return;
      setTabs((prev) =>
        prev.map((tab) =>
          tab.id === activeTab.id ? { ...tab, cursor: next, mode: "read" } : tab,
        ),
      );
    },
    [activeTab, guardEdit],
  );

  const switchTab = useCallback(
    (id: string) => {
      if (id === activeTabId) return;
      if (!guardEdit(activeTab)) return;
      setActiveTabId(id);
    },
    [activeTab, activeTabId, guardEdit],
  );

  const addHomeTab = useCallback(() => {
    const tab = freshHomeTab();
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
  }, []);

  const closeTab = useCallback(
    (id: string) => {
      const index = tabs.findIndex((tab) => tab.id === id);
      if (index < 0) return;
      const remaining = tabs.filter((tab) => tab.id !== id);
      const next = remaining.length > 0 ? remaining : [freshHomeTab()];
      setTabs(next);
      if (activeTabId === id) {
        setActiveTabId(next[Math.min(index, next.length - 1)].id);
      }
    },
    [activeTabId, tabs],
  );

  // dock/阅读器发出的全局打开事件:当前是词条 tab 就地压栈,否则新开。
  useEffect(() => {
    const handler = (event: Event) => {
      const id = (event as CustomEvent<string>).detail;
      if (typeof id !== "string") return;
      openEntry(id, "", current.kind !== "entry");
    };
    window.addEventListener("wiki:open-entry", handler);
    return () => window.removeEventListener("wiki:open-entry", handler);
  }, [current.kind, openEntry]);

  const refreshBrowse = useCallback(
    () => loadBrowse(tagFilter),
    [loadBrowse, tagFilter],
  );

  async function createNew() {
    setCreating(true);
    try {
      const base = `Untitled ${new Date().toLocaleString()}`;
      const created = await createEntry({
        title: base,
        content: `# ${base}\n\n`,
        tags: tagFilter ? parseTags(tagFilter) : [],
      });
      // 新词条开新 tab,直接进编辑模式;首页浏览位置不动。
      const id = crypto.randomUUID();
      const loc: TabLocation = { kind: "entry", entryId: created.id, title: base };
      setTabs((prev) => [...prev, { id, stack: [loc], cursor: 0, mode: "edit" }]);
      setActiveTabId(id);
      void refreshBrowse();
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setCreating(false);
    }
  }

  async function softDelete() {
    if (current.kind !== "entry") return;
    await deleteEntry(current.entryId);
    toast.success("已移入回收站(标题在净删前仍被占用)");
    closeTab(activeTab.id);
    void refreshBrowse();
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
      void refreshBrowse();
    } catch (error) {
      toast.error((error as Error).message);
    }
  }

  async function createMissingLink() {
    if (!missingLink) return;
    setCreatingLink(true);
    try {
      const created = await createEntry({
        title: missingLink,
        content: `# ${missingLink}\n\n`,
        tags: tagFilter ? parseTags(tagFilter) : [],
      });
      toast.success(`已创建「${missingLink}」`);
      setMissingLink(null);
      openEntry(created.id, missingLink, false);
      void refreshBrowse();
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setCreatingLink(false);
    }
  }

  function setMode(mode: Mode) {
    setTabs((prev) =>
      prev.map((tab) => (tab.id === activeTab.id ? { ...tab, mode } : tab)),
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* 工具栏:全局动作 + 当前 tab 的前进/后退 */}
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <span className="flex items-center gap-1.5 text-sm font-semibold">
          <BookOpen className="h-4 w-4" /> 知识库
        </span>
        <div className="flex items-center gap-0.5">
          <Button
            size="icon-sm"
            variant="ghost"
            title="后退"
            disabled={!canBack}
            onClick={() => stepHistory(-1)}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
          <Button
            size="icon-sm"
            variant="ghost"
            title="前进"
            disabled={!canForward}
            onClick={() => stepHistory(1)}
          >
            <ArrowRight className="h-4 w-4" />
          </Button>
        </div>
        <div className="flex-1" />
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

      {/* 标签栏 */}
      <div
        className="flex items-center gap-1 overflow-x-auto border-b px-2 py-1"
        role="tablist"
        aria-label="wiki 标签页"
      >
        {tabs.map((tab) => {
          const loc = tab.stack[tab.cursor];
          const active = tab.id === activeTabId;
          return (
            <div
              key={tab.id}
              role="tab"
              aria-selected={active}
              tabIndex={0}
              className={`group flex max-w-52 shrink-0 cursor-pointer items-center gap-1.5 rounded-t-md border border-b-0 px-2.5 py-1.5 text-xs transition-colors ${
                active
                  ? "border-border bg-background text-foreground"
                  : "border-transparent text-muted-foreground hover:bg-accent"
              }`}
              onClick={() => switchTab(tab.id)}
              onAuxClick={(event) => {
                if (event.button === 1) {
                  event.preventDefault();
                  closeTab(tab.id);
                }
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") switchTab(tab.id);
              }}
            >
              {loc.kind === "home" ? (
                <Search className="h-3 w-3 shrink-0" />
              ) : (
                <FileText className="h-3 w-3 shrink-0" />
              )}
              <span className="min-w-0 truncate">
                {loc.kind === "home" ? "首页" : loc.title}
              </span>
              <button
                type="button"
                title="关闭标签页"
                className="rounded p-0.5 opacity-0 transition-opacity hover:bg-accent group-hover:opacity-100"
                onClick={(event) => {
                  event.stopPropagation();
                  closeTab(tab.id);
                }}
                onKeyDown={(event) => event.stopPropagation()}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          );
        })}
        <button
          type="button"
          title="新建标签页"
          className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-accent"
          onClick={addHomeTab}
        >
          <Plus className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* 当前 tab 内容 */}
      <main className="min-h-0 flex-1 overflow-hidden p-3">
        {current.kind === "home" ? (
          <HomeView
            entries={entries}
            loading={loading}
            hasMore={hasMore}
            loadingMore={loadingMore}
            query={query}
            tagFilter={tagFilter}
            onQueryChange={setQuery}
            onTagFilterChange={setTagFilter}
            onSearch={() => void loadSearch(query, tagFilter)}
            onBrowse={() => void loadBrowse(tagFilter)}
            onLoadMore={() => void loadBrowse(tagFilter, entries.length)}
            onOpen={openEntry}
          />
        ) : (
          <EntryTabContent
            key={`${activeTab.id}:${current.entryId}`}
            entryId={current.entryId}
            fetchEntry={fetchEntry}
            mode={activeTab.mode}
            onModeChange={setMode}
            onOpenEntry={(id, newTab) => openEntry(id, "", newTab)}
            onMissingLink={setMissingLink}
            onDelete={() => void softDelete()}
            onSaved={(entry) => {
              applyEntry(entry);
              setMode("read");
              void refreshBrowse();
            }}
            dockOpen={dockOpen}
            onToggleDock={toggleDock}
          />
        )}
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

      <Dialog open={missingLink !== null} onOpenChange={(open) => {
        if (!open) setMissingLink(null);
      }}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>词条不存在</DialogTitle>
            <DialogDescription>
              「{missingLink}」尚未创建。创建并打开它,或返回继续阅读?
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setMissingLink(null)}>
              取消
            </Button>
            <Button onClick={() => void createMissingLink()} disabled={creatingLink}>
              {creatingLink ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              创建
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** 词条 tab 的正文:按 entryId 取数(带缓存),渲染阅读/编辑 + 底部 dock。 */
function EntryTabContent({
  entryId,
  fetchEntry,
  mode,
  onModeChange,
  onOpenEntry,
  onMissingLink,
  onDelete,
  onSaved,
  dockOpen,
  onToggleDock,
}: {
  entryId: string;
  fetchEntry: (id: string) => Promise<WikiEntry | null>;
  mode: Mode;
  onModeChange: (mode: Mode) => void;
  onOpenEntry: (id: string, newTab: boolean) => void;
  onMissingLink: (target: string) => void;
  onDelete: () => void;
  onSaved: (entry: WikiEntry) => void;
  dockOpen: boolean;
  onToggleDock: () => void;
}) {
  // undefined = 取数中;取不到会转入 missing 分支。
  const [entry, setEntry] = useState<WikiEntry | undefined>(undefined);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setMissing(false);
    setEntry(undefined);
    void fetchEntry(entryId).then((found) => {
      if (cancelled) return;
      if (found === null) setMissing(true);
      else setEntry(found);
    });
    return () => {
      cancelled = true;
    };
  }, [entryId, fetchEntry]);

  if (missing) {
    return (
      <div className="flex h-full items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground">
        词条不存在或已被删除。
      </div>
    );
  }
  if (entry === undefined) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        <Loader2 className="h-5 w-5 animate-spin" />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      {mode === "edit" ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <EditorView
            key={entry.id}
            entry={entry}
            onModeChange={onModeChange}
            onSaved={(saved) => {
              // 本地正文同步更新,否则切回阅读态展示的是保存前的内容。
              setEntry(saved);
              onSaved(saved);
            }}
          />
        </div>
      ) : (
        <>
          <div className="flex min-h-0 flex-1 flex-col">
            <ReadingView
              entry={entry}
              onModeChange={onModeChange}
              onDelete={onDelete}
              onOpenEntry={onOpenEntry}
              onMissingLink={onMissingLink}
            />
          </div>
          {/* 折叠区块:链接 / 附件 / 历史 */}
          <div className="rounded-md border">
            <button
              type="button"
              className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent"
              onClick={onToggleDock}
            >
              {dockOpen ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
              链接与历史
            </button>
            {dockOpen ? (
              <div className="h-64 overflow-hidden border-t px-1 pb-1">
                <SidePanel entry={entry} />
              </div>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}

function SidePanel({ entry }: { entry: WikiEntry }) {
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
  const [mentions, setMentions] = useState<WikiLinkRef[] | null>(null);

  useEffect(() => {
    setGraph(null);
    setMentions(null);
    void getLinks(entry.id).then(setGraph).catch((error: Error) => toast.error(error.message));
    void getMentions(entry.id).then(setMentions).catch(() => setMentions([]));
  }, [entry.id]);

  if (graph === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  return (
    <div className="space-y-3 text-xs">
      <LinkSection title={`出链(${graph.out_links.length})`} refs={graph.out_links} onOpen={openEntryById} />
      <LinkSection title={`入链(${graph.in_links.length})`} refs={graph.in_links} onOpen={openEntryById} />
      <LinkSection
        title={`未链接提及(${mentions?.length ?? 0})`}
        refs={mentions ?? []}
        onOpen={openEntryById}
        empty={mentions !== null ? "无 —— 在其他词条中添加 [[链接]] 后这里会出现候选。" : undefined}
        loading={mentions === null}
      />
    </div>
  );
}

/** 侧栏点击 → 通知面板打开词条(与阅读器双链共用一条事件通道)。 */
function openEntryById(id: string) {
  window.dispatchEvent(new CustomEvent("wiki:open-entry", { detail: id }));
}

function LinkSection({
  title,
  refs,
  onOpen,
  empty,
  loading,
}: {
  title: string;
  refs: WikiLinkRef[];
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
          ) : refs.length === 0 ? (
            <p className="text-muted-foreground">{empty ?? "无"}</p>
          ) : (
            refs.map((ref) => (
              <button
                key={ref.id}
                type="button"
                className="flex w-full items-center gap-1.5 truncate rounded px-1.5 py-1 text-left hover:bg-accent"
                title={ref.title}
                onClick={() => onOpen(ref.id)}
              >
                <Link2 className="h-3 w-3 shrink-0 text-muted-foreground" />
                <span className="truncate">{ref.title || ref.id}</span>
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
