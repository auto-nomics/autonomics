/**
 * Wiki entry editor surface: the body of one workbench editor tab. Fetches
 * the entry through the shared cache, renders Reading/Editing views and
 * the collapsible links/attachments/history dock. Navigation (wikilinks,
 * dock rows, missing-link creation) is delegated to the host via
 * `onNavigate` — the host owns the per-tab history stack.
 */

import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, Loader2, Plus } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EditorView } from "./editor-view";
import { ReadingView, type WikiMode } from "./reading-view";
import {
  attachmentRawUrl,
  createEntry,
  deleteEntry,
  detachAttachment,
  formatTime,
  getLinks,
  getMentions,
  listAttachments,
  listVersions,
  restoreVersion,
  uploadAttachment,
  type AttachmentMeta,
  type WikiEntry,
  type WikiLinkRef,
  type VersionSummary,
} from "./wiki-api";
import {
  bumpWikiRevision,
  cacheEntry,
  cachedTitle,
  fetchEntryCached,
  resolveTitleCached,
} from "./wiki-state";

export function WikiEntrySurface({
  entryId,
  mode,
  onModeChange,
  onNavigate,
  onSaved,
  onDeleted,
}: {
  entryId: string;
  mode: WikiMode;
  onModeChange: (mode: WikiMode) => void;
  /** Navigation from inside the surface: reading-view links pass
   *  `newTab` (ctrl/middle click); dock rows pass a title. The host owns
   *  the per-tab history stack and decides push vs. new editor tab. */
  onNavigate: (entryId: string, title?: string, newTab?: boolean) => void;
  onSaved?: (entry: WikiEntry) => void;
  onDeleted?: () => void;
}) {
  // undefined = fetching; null = missing.
  const [entry, setEntry] = useState<WikiEntry | undefined | null>(undefined);
  const [missingLink, setMissingLink] = useState<string | null>(null);
  const [creatingLink, setCreatingLink] = useState(false);
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

  useEffect(() => {
    let cancelled = false;
    setEntry(undefined);
    void fetchEntryCached(entryId).then((found) => {
      if (cancelled) return;
      setEntry(found);
    });
    return () => {
      cancelled = true;
    };
  }, [entryId]);

  if (entry === null) {
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
    <div className="flex min-h-0 flex-1 flex-col">
      {mode === "edit" ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <EditorView
            key={entry.id}
            entry={entry}
            onModeChange={onModeChange}
            onSaved={(saved) => {
              cacheEntry(saved);
              onModeChange("read");
              bumpWikiRevision();
              onSaved?.(saved);
            }}
          />
        </div>
      ) : (
        <>
          <div className="flex min-h-0 flex-1 flex-col">
            <ReadingView
              entry={entry}
              onModeChange={onModeChange}
              onDelete={() =>
                void (async () => {
                  try {
                    await deleteEntry(entry.id);
                    toast.success("已移入回收站(标题在净删前仍被占用)");
                    bumpWikiRevision();
                    onDeleted?.();
                  } catch (error) {
                    toast.error((error as Error).message);
                  }
                })()
              }
              onOpenEntry={(id, newTab) => onNavigate(id, undefined, newTab)}
              onMissingLink={setMissingLink}
            />
          </div>
          {/* 折叠区块:链接 / 附件 / 历史(密铺式,上缘细线分隔) */}
          <div className="border-t">
            <button
              type="button"
              className="flex w-full items-center gap-1.5 px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent/70"
              onClick={toggleDock}
            >
              {dockOpen ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
              链接与历史
            </button>
            {dockOpen ? (
              <div className="h-64 overflow-hidden border-t px-1 pb-1">
                <SidePanel entry={entry} onNavigate={onNavigate} />
              </div>
            ) : null}
          </div>
        </>
      )}

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
            <Button
              onClick={() =>
                void (async () => {
                  if (!missingLink) return;
                  setCreatingLink(true);
                  try {
                    const created = await createEntry({
                      title: missingLink,
                      content: `# ${missingLink}\n\n`,
                      tags: [],
                    });
                    toast.success(`已创建「${missingLink}」`);
                    setMissingLink(null);
                    bumpWikiRevision();
                    onNavigate(created.id, missingLink);
                  } catch (error) {
                    toast.error((error as Error).message);
                  } finally {
                    setCreatingLink(false);
                  }
                })()
              }
              disabled={creatingLink}
            >
              {creatingLink ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              创建
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function SidePanel({
  entry,
  onNavigate,
}: {
  entry: WikiEntry;
  onNavigate: (entryId: string, title?: string) => void;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <Tabs defaultValue="links" className="flex h-full min-h-0 flex-col gap-2">
        <TabsList className="grid w-full grid-cols-3">
          <TabsTrigger value="links">链接</TabsTrigger>
          <TabsTrigger value="files">附件</TabsTrigger>
          <TabsTrigger value="history">历史</TabsTrigger>
        </TabsList>
        <TabsContent value="links" className="min-h-0 flex-1 overflow-y-auto">
          <LinksTab entry={entry} onNavigate={onNavigate} />
        </TabsContent>
        <TabsContent value="files" className="min-h-0 flex-1 overflow-y-auto">
          <FilesTab entry={entry} />
        </TabsContent>
        <TabsContent value="history" className="min-h-0 flex-1 overflow-y-auto">
          <HistoryTab entry={entry} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function LinksTab({
  entry,
  onNavigate,
}: {
  entry: WikiEntry;
  onNavigate: (entryId: string, title?: string) => void;
}) {
  const [graph, setGraph] = useState<Awaited<ReturnType<typeof getLinks>> | null>(null);
  const [mentions, setMentions] = useState<WikiLinkRef[] | null>(null);
  const [, setResolvedTick] = useState(0);

  useEffect(() => {
    setGraph(null);
    setMentions(null);
    void getLinks(entry.id).then(setGraph).catch((error: Error) => toast.error(error.message));
    void getMentions(entry.id).then(setMentions).catch(() => setMentions([]));
  }, [entry.id]);

  // 客户端标题补全:normalizeRefs 的兜底是"显示 id";这里把仍等于 id 的
  // 行再解析一次,拿到标题后写缓存并触发重渲染。
  useEffect(() => {
    let cancelled = false;
    const refs = normalizeRefs([
      ...(graph?.out_links ?? []),
      ...(graph?.in_links ?? []),
      ...(mentions ?? []),
    ]).map((ref) => ({ id: ref.id, title: ref.title || ref.id }));
    const unknown = [...new Set(refs.filter((ref) => ref.title === ref.id).map((ref) => ref.id))];
    if (!unknown.length) return;
    void Promise.all(
      unknown.map(async (id) => {
        const title = await resolveTitleCached(id);
        if (title && title !== id) {
          if (!cancelled) setResolvedTick((tick) => tick + 1);
        }
      }),
    );
    return () => {
      cancelled = true;
    };
  }, [graph, mentions]);

  if (graph === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  const withTitles = (refs: WikiLinkRef[]) =>
    refs.map((ref) => ({ ...ref, title: cachedTitle(ref.id) ?? (ref.title || ref.id) }));
  return (
    <div className="space-y-3 text-xs">
      <LinkSection title={`出链(${graph.out_links.length})`} refs={withTitles(normalizeRefs(graph.out_links))} onOpen={(id, title) => onNavigate(id, title)} />
      <LinkSection title={`入链(${graph.in_links.length})`} refs={withTitles(normalizeRefs(graph.in_links))} onOpen={(id, title) => onNavigate(id, title)} />
      <LinkSection
        title={`未链接提及(${mentions?.length ?? 0})`}
        refs={withTitles(normalizeRefs(mentions ?? []))}
        onOpen={(id, title) => onNavigate(id, title)}
        empty={mentions !== null ? "无 —— 在其他词条中添加 [[链接]] 后这里会出现候选。" : undefined}
        loading={mentions === null}
      />
    </div>
  );
}

/**
 * 兼容两种 REST 形状:新 daemon 返回 {id,title}(title 缺省回退 id),
 * 旧 daemon 返回裸 id 字符串数组 —— 旧形状不做归一会让 dock 行空白且
 * 点击失效(ref.title/ref.id 双双 undefined)。行点击始终按 id 路由。
 */
function normalizeRefs(refs: unknown): WikiLinkRef[] {
  if (!Array.isArray(refs)) return [];
  return refs.flatMap((ref): WikiLinkRef[] => {
    if (typeof ref === "string") return [{ id: ref, title: ref }];
    if (ref !== null && typeof ref === "object" && typeof (ref as { id?: unknown }).id === "string") {
      const { id, title } = ref as { id: string; title?: unknown };
      return [{ id, title: typeof title === "string" && title ? title : id }];
    }
    return [];
  });
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
  onOpen: (id: string, title: string) => void;
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
                onClick={() => onOpen(ref.id, ref.title || ref.id)}
              >
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
        上传图像
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
