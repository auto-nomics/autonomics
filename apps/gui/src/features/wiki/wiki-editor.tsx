/**
 * Wiki entry editor surface: the body of one workbench editor tab. Fetches
 * the entry through the shared cache, renders Reading/Editing views and
 * the collapsible links/attachments/history dock. Navigation (wikilinks,
 * dock rows, missing-link creation) is delegated to the host via
 * `onNavigate` — the host owns the per-tab history stack.
 */

import { insertWikiImage } from "./wiki-document";
import { wikiImageMarkdown } from "@/lib/wiki-attachment-ref";
import { WikiAttachmentImage } from "./wiki-images";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Loader2, Plus, Search } from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { EditorView } from "./editor-view";
import { ReadingView, type WikiMode } from "./reading-view";
import {
  createEntry,
  deleteEntry,
  detachAttachment,
  formatTime,
  getLinks,
  getEntry,
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
  useWikiRevision,
} from "./wiki-state";

export function WikiEntrySurface({
  entryId,
  navigation,
  mode,
  onModeChange,
  onNavigate,
  onSaved,
  onDeleted,
}: {
  entryId: string;
  navigation?: ReactNode;
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
  const { t } = useTranslation("wiki");
  const [missingLink, setMissingLink] = useState<string | null>(null);
  const [creatingLink, setCreatingLink] = useState(false);
  const revision = useWikiRevision();
  useEffect(() => {
    if (!revision) return;
    let cancelled = false;
    void fetchEntryCached(entryId).then(found => { if (!cancelled) setEntry(found); });
    return () => { cancelled = true; };
  }, [entryId, revision]);

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
      <div className="flex h-full flex-col text-sm text-muted-foreground">
        <div className="flex items-center p-2">{navigation}</div>
        <div className="grid flex-1 place-items-center">{t("surface.missing")}</div>
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
            navigation={navigation}
            onModeChange={onModeChange}
            onSaved={(saved, stayEditing) => {
              cacheEntry(saved);
              if (!stayEditing) onModeChange("read");
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
              navigation={navigation}
              footer={<SidePanel entry={entry} onNavigate={onNavigate} onInsert={meta => {
                insertWikiImage(entry, wikiImageMarkdown(entry.id, meta.attachment_id ?? meta.id ?? "", meta.filename));
                onModeChange("edit");
              }} />}
              onModeChange={onModeChange}
              onDelete={() =>
                void (async () => {
                  try {
                    await deleteEntry(entry.id);
                    toast.success(t("surface.recycleToast"));
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
        </>
      )}

      <Dialog open={missingLink !== null} onOpenChange={(open) => {
        if (!open) setMissingLink(null);
      }}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("surface.missingTitle")}</DialogTitle>
            <DialogDescription>
              {t("surface.missingDescription", { title: missingLink ?? "" })}
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setMissingLink(null)}>
              {t("common.cancel")}
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
                    toast.success(t("surface.createdToast", { title: missingLink }));
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
              {t("surface.create")}
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
  onInsert,
}: {
  entry: WikiEntry;
  onNavigate: (entryId: string, title?: string) => void;
  onInsert: (meta: AttachmentMeta) => void;
}) {
  const { t } = useTranslation("wiki");
  return (
    <div className="mx-auto mt-12 max-w-3xl border-t pb-8 pt-4 text-sm" aria-label={t("surface.panelAria")}>
      <LinksTab entry={entry} onNavigate={onNavigate} />
      <DetailSection title={t("surface.attachments")}><FilesTab entry={entry} onInsert={onInsert} /></DetailSection>
      <DetailSection title={t("surface.history")}><HistoryTab entry={entry} /></DetailSection>
    </div>
  );
}

function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return <details className="mt-4 border-t py-3 text-muted-foreground" onToggle={event => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer font-medium hover:text-foreground">{title}</summary>
    {open && <div className="pt-3 text-foreground">{children}</div>}
  </details>;
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
  const [error, setError] = useState(false);
  const [filter, setFilter] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [descending, setDescending] = useState(false);
  const [contexts, setContexts] = useState<Record<string, MentionContext | null>>({});
  const [, setResolvedTick] = useState(0);
  const { t } = useTranslation("wiki");

  useEffect(() => {
    let cancelled = false;
    setGraph(null); setMentions(null); setError(false);
    void getLinks(entry.id).then(value => { if (!cancelled) setGraph(value); }).catch(() => { if (!cancelled) setError(true); });
    void getMentions(entry.id).then(value => { if (!cancelled) setMentions(value); }).catch(() => { if (!cancelled) setMentions([]); });
    return () => { cancelled = true; };
  }, [entry.id]);

  useEffect(() => {
    let cancelled = false;
    const refs = normalizeRefs([...(graph?.out_links ?? []), ...(graph?.in_links ?? []), ...(mentions ?? [])]);
    const unknown = [...new Set(refs.filter(ref => ref.title === ref.id).map(ref => ref.id))];
    void Promise.all(unknown.map(id => resolveTitleCached(id))).then(() => { if (!cancelled && unknown.length) setResolvedTick(tick => tick + 1); });
    return () => { cancelled = true; };
  }, [graph, mentions]);

  useEffect(() => {
    if (!graph) return;
    let cancelled = false;
    const ids = [...new Set(normalizeRefs(graph.in_links).map(ref => ref.id))];
    let next = 0;
    setContexts({});
    const load = async () => {
      while (!cancelled && next < ids.length) {
        const id = ids[next++];
        try {
          const source = await getEntry(id);
          if (!cancelled) setContexts(previous => ({ ...previous, [id]: findMentionContext(source.content, entry) }));
        } catch {
          if (!cancelled) setContexts(previous => ({ ...previous, [id]: null }));
        }
      }
    };
    void Promise.all(Array.from({ length: Math.min(6, ids.length) }, load));
    return () => { cancelled = true; };
  }, [graph, entry]);

  const withTitles = (refs: unknown) => normalizeRefs(refs).map(ref => ({ ...ref, title: cachedTitle(ref.id) ?? ref.title }));
  const incoming = withTitles(graph?.in_links ?? []);
  const visible = incoming.filter(ref => ref.title.toLocaleLowerCase().includes(filter.toLocaleLowerCase())).sort((a, b) => (descending ? -1 : 1) * a.title.localeCompare(b.title, undefined, { numeric: true }));
  return <div>
    <div className="flex items-center gap-2">
      <h2 className="font-semibold">{t("links.related")}</h2>
      <span className="text-xs text-muted-foreground">{graph ? incoming.length : ""}</span>
      <div className="ml-auto flex items-center gap-1 text-muted-foreground">
        <button type="button" aria-label={t("links.toggleSortAria")} title={t("links.toggleSort")} className="rounded px-1.5 py-1 hover:bg-accent hover:text-foreground" onClick={() => setDescending(value => !value)}>{descending ? "Z → A" : "A → Z"}</button>
        <button type="button" aria-label={t("links.filterAria")} title={t("links.filterAria")} className="rounded p-1 hover:bg-accent hover:text-foreground" onClick={() => setSearchOpen(value => !value)}><Search className="size-4" /></button>
      </div>
    </div>
    {searchOpen && <Input autoFocus aria-label={t("links.filterPlaceholder")} value={filter} onChange={event => setFilter(event.target.value)} placeholder={t("links.filterPlaceholder")} className="my-2 h-8 text-xs" />}
    {error ? <p className="py-3 text-xs text-destructive">{t("links.loadError")}</p> : !graph ? <Loader2 className="my-3 size-4 animate-spin" /> : visible.length === 0 ? <p className="py-3 text-xs text-muted-foreground">{filter ? t("links.noMatchSources") : t("links.none")}</p> : <div className="mt-2 space-y-1">{visible.map(ref => <MentionRow key={ref.id} entry={entry} source={ref} context={contexts[ref.id]} onNavigate={onNavigate} />)}</div>}
    <div className="mt-4 space-y-3 border-t pt-3 text-xs text-muted-foreground">
      <LinkSection title={t("links.outgoing", { count: graph ? withTitles(graph.out_links).length : "…" })} refs={withTitles(graph?.out_links ?? [])} onOpen={(id, title) => onNavigate(id, title)} loading={!graph && !error} />
      <LinkSection title={t("links.unlinked", { count: mentions?.length ?? 0 })} refs={withTitles(mentions ?? [])} onOpen={(id, title) => onNavigate(id, title)} empty={t("links.noUnlinked")} loading={mentions === null} />
    </div>
  </div>;
}

type MentionContext = { lines: string[]; count: number };

function isLinkToEntry(link: string, entry: WikiEntry): boolean {
  const target = link.split("|")[0].split("#")[0].trim().toLocaleLowerCase();
  return [entry.id, entry.title, ...entry.alias].some(value => value.trim().toLocaleLowerCase() === target);
}

function findMentionContext(content: string, entry: WikiEntry): MentionContext {
  const lines: string[] = [];
  let count = 0;
  for (const line of content.split("\n")) {
    const matches = Array.from(line.matchAll(/\[\[([^\]]+)\]\]/g)).filter(match => isLinkToEntry(match[1], entry));
    if (matches.length) { lines.push(line); count += matches.length; }
  }
  return { lines, count };
}

function MentionRow({ entry, source, context, onNavigate }: { entry: WikiEntry; source: WikiLinkRef; context: MentionContext | null | undefined; onNavigate: (id: string, title?: string) => void }) {
  const [open, setOpen] = useState(false);
  const { t } = useTranslation("wiki");
  return <div>
    <div className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
      <button type="button" aria-label={open ? t("links.collapseOf", { title: source.title }) : t("links.expandOf", { title: source.title })} aria-expanded={open} className="rounded p-1 hover:bg-accent" onClick={() => setOpen(value => !value)}>{open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}</button>
      <button type="button" className="min-w-0 flex-1 truncate py-1 text-left hover:underline" title={source.title} onClick={() => onNavigate(source.id, source.title)}>{source.title}</button>
      {context && context.count > 0 && <span className="px-1 text-xs" aria-label={t("links.mentionCountAria", { title: source.title, count: context.count })}>{context.count}</span>}
      {context === null && <span className="px-1 text-xs" title={t("links.countUnavailableTitle")} aria-label={t("links.countUnavailableAria", { title: source.title })}>—</span>}
    </div>
    {open && <div className="space-y-1 pb-1 pl-1">{context === null ? <p className="px-2 text-xs text-muted-foreground">{t("links.loadSourceError")}</p> : context === undefined ? <Loader2 className="m-2 size-3.5 animate-spin" /> : context.lines.length === 0 ? <p className="px-2 text-xs text-muted-foreground">{t("links.noContext")}</p> : context.lines.map((line, index) => <button key={index} type="button" className="block w-full rounded border px-3 py-2 text-left text-xs leading-relaxed text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => onNavigate(source.id, source.title)}>{line.split(/(\[\[[^\]]+\]\])/g).map((part, partIndex) => part.startsWith("[[") && part.endsWith("]]") && isLinkToEntry(part.slice(2, -2), entry) ? <mark key={partIndex} className="rounded-sm bg-yellow-500/30 text-foreground">{part}</mark> : part)}</button>)}</div>}
  </div>;
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
  const { t } = useTranslation("wiki");
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
            <p className="text-muted-foreground">{empty ?? t("links.noneDefault")}</p>
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

function FilesTab({ entry, onInsert }: { entry: WikiEntry; onInsert: (meta: AttachmentMeta) => void }) {
  const [items, setItems] = useState<AttachmentMeta[] | null>(null);
  const { t } = useTranslation("wiki");
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
              toast.success(t("attachments.attached", { name: file.name }));
              await reload();
            } catch (error) {
              toast.error((error as Error).message);
            }
          })();
        }}
      />
      <Button size="sm" variant="outline" className="w-full" onClick={() => fileInput.current?.click()}>
        {t("attachments.upload")}
      </Button>
      {items === null ? (
        <Loader2 className="h-4 w-4 animate-spin" />
      ) : items.length === 0 ? (
        <p className="text-muted-foreground">{t("attachments.none")}</p>
      ) : (
        <ul className="grid grid-cols-2 gap-2">
          {items.map((item) => {
            const id = item.attachment_id ?? item.id ?? "";
            return (
              <li key={id} className="rounded-md border p-1.5">
                <WikiAttachmentImage entryId={entry.id} attachmentId={id} filename={item.filename} />
                <button type="button" className="mt-1 underline" onClick={() => onInsert(item)}>{t("attachments.insert")}</button>
                <div className="mt-1 flex items-center justify-between gap-1">
                  <span className="truncate text-[11px]">{item.filename}</span>
                  <button
                    type="button"
                    className="rounded p-0.5 hover:bg-accent"
                    title={t("attachments.detach")}
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
  const { t } = useTranslation("wiki");

  useEffect(() => {
    setVersions(null);
    void listVersions(entry.id).then(setVersions).catch(() => setVersions([]));
  }, [entry.id]);

  if (versions === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  if (!versions.length) {
    return <p className="text-xs text-muted-foreground">{t("history.none")}</p>;
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
                  toast.success(t("history.rolledBack", { version: version.version }));
                } catch (error) {
                  toast.error((error as Error).message);
                }
              })()
            }
          >
            {t("history.rollback")}
          </Button>
        </li>
      ))}
    </ul>
  );
}
