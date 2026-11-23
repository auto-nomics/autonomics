import {
  ArrowLeft,
  ArrowUpRight,
  Check,
  Inbox,
  Library,
  Plus,
  PanelLeft,
  RefreshCw,
  Rss,
  Search,
  Trash2,
} from "lucide-react";
import "./rss.css";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { createPortal } from "react-dom";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { BibCollection } from "@/types";
import { SearchDialog } from "@/features/bibliography";

export interface FeedSpec {
  name: string;
  url: string;
  enabled: boolean;
  interval_seconds: number;
  max_items_per_feed: number;
}
export interface Feed {
  id: string;
  version: number;
  spec: FeedSpec;
  etag: string | null;
  last_modified: string | null;
  last_checked_ms: number | null;
  last_success_ms: number | null;
  last_error: string | null;
  next_due_ms: number;
  item_count: number;
  unread_count: number;
}
export interface FeedItem {
  id: string;
  feed_id: string;
  guid: string;
  title: string;
  link: string | null;
  summary: string | null;
  authors: string[];
  published_ms: number | null;
  first_seen_ms: number;
  read: boolean;
  imported_article_id: string | null;
}
export interface CheckOutcome {
  feed_id: string;
  status: "fetched" | "not_modified" | "error";
  new_items: number;
  total_items: number;
  error: string | null;
}
export interface SchedulerStatus {
  running: boolean;
  active_checks: number;
  last_error: string | null;
}

const api = (suffix = "") => `/bib/rss${suffix}`;
const path = (id: string) => `/${encodeURIComponent(id)}`;
const feedPath = (id: string) => `/feeds${path(id)}`;
const minutes = (seconds: number) => Math.round(seconds / 60);
const defaultSpec = (): FeedSpec => ({
  name: "",
  url: "",
  enabled: true,
  interval_seconds: 3600,
  max_items_per_feed: 500,
});

function FeedEditor({
  initial,
  title,
  onClose,
  onSave,
}: {
  initial: FeedSpec;
  title: string;
  onClose: () => void;
  onSave: (spec: FeedSpec) => Promise<void>;
}) {
  const { t } = useTranslation("discovery");
  const [draft, setDraft] = useState(() => structuredClone(initial));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const update = <K extends keyof FeedSpec>(key: K, value: FeedSpec[K]) =>
    setDraft((old) => ({ ...old, [key]: value }));
  async function save() {
    setBusy(true);
    setError("");
    try {
      const name = draft.name.trim();
      const url = draft.url.trim();
      if (!name || !safeLink(url)) throw new Error(t("rss.errNameUrl"));
      if (
        !Number.isInteger(draft.interval_seconds) ||
        draft.interval_seconds < 300 ||
        draft.interval_seconds > 2_592_000
      )
        throw new Error(t("rss.errInterval"));
      if (
        !Number.isInteger(draft.max_items_per_feed) ||
        draft.max_items_per_feed < 1 ||
        draft.max_items_per_feed > 5000
      )
        throw new Error(t("rss.errRetention"));
      await onSave({ ...draft, name, url });
    } catch (failure) {
      setError(String(failure));
    } finally {
      setBusy(false);
    }
  }
  return (
    <SearchDialog
      title={title}
      onClose={() => {
        if (!busy) onClose();
      }}
      footer={
        <div className="flex justify-end gap-2">
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            {t("rss.cancel")}
          </Button>
          <Button size="sm" disabled={busy} onClick={() => void save()}>
            {busy ? t("rss.saving") : t("rss.saveFeed")}
          </Button>
        </div>
      }
    >
      <fieldset disabled={busy} className="space-y-3">
        <label className="block text-xs">
          {t("rss.feedName")}
          <Input
            aria-label={t("rss.feedName")}
            value={draft.name}
            maxLength={256}
            onChange={(e) => update("name", e.target.value)}
          />
        </label>
        <label className="block text-xs">
          {t("rss.feedUrl")}
          <Input
            aria-label={t("rss.feedUrlAria")}
            placeholder="https://…"
            value={draft.url}
            onChange={(e) => update("url", e.target.value)}
          />
        </label>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => update("enabled", e.target.checked)}
          />
          {t("rss.enableAutomaticChecks")}
        </label>
        <div className="grid grid-cols-2 gap-3 text-xs">
          <label>
            {t("rss.checkInterval")}
            <Input
              aria-label={t("rss.checkInterval")}
              type="number"
              min={5}
              max={43200}
              value={minutes(draft.interval_seconds)}
              onChange={(e) =>
                update("interval_seconds", Number(e.target.value) * 60)
              }
            />
          </label>
          <label>
            {t("rss.retainedItems")}
            <Input
              aria-label={t("rss.retainedItems")}
              type="number"
              min={1}
              max={5000}
              value={draft.max_items_per_feed}
              onChange={(e) =>
                update("max_items_per_feed", Number(e.target.value))
              }
            />
          </label>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("rss.backgroundNote")}
        </p>
      </fieldset>
      {error && (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {error}
        </p>
      )}
    </SearchDialog>
  );
}

export function RssSubscriptions({
  active = true,
  sidebarSlot,
  externalSidebar = false,
  onOpenSidebar,
}: {
  active?: boolean;
  sidebarSlot?: HTMLElement | null;
  externalSidebar?: boolean;
  onOpenSidebar?: () => void;
}) {
  const { t } = useTranslation("discovery");
  const date = (ms: number | null) =>
    ms === null ? t("rss.never") : new Date(ms).toLocaleString();
  const [feeds, setFeeds] = useState<Feed[]>([]);
  const [items, setItems] = useState<FeedItem[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [scheduler, setScheduler] = useState<SchedulerStatus | null>(null);
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [collection, setCollection] = useState("");
  const [filterFeed, setFilterFeed] = useState("");
  const [editor, setEditor] = useState<{
    initial: FeedSpec;
    id?: string;
    version?: number;
  } | null>(null);
  const [confirm, setConfirm] = useState<Feed | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const loadedPages = useRef(1);
  const [showFeeds, setShowFeeds] = useState(false);
  const [query, setQuery] = useState("");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const lifecycle = useRef<AbortController | null>(null);
  const mutation = useRef(false);
  const loadSeq = useRef(0);
  const filterRef = useRef("");
  filterRef.current = filterFeed;

  async function fetchAll(signal: AbortSignal) {
    const sequence = ++loadSeq.current;
    const itemPath = filterRef.current
      ? `/items?feed=${encodeURIComponent(filterRef.current)}&limit=50`
      : "/items?limit=50";
    const [feedData, firstPage, status] = await Promise.all([
      gatewayRequest("GET", api("/feeds"), undefined, { signal }),
      gatewayRequest("GET", api(itemPath), undefined, { signal }),
      gatewayRequest("GET", api("/status"), undefined, { signal }),
    ]);
    const itemData = { ...firstPage, items: [...(firstPage.items ?? [])] };
    for (
      let page = 1;
      page < loadedPages.current &&
      itemData.next !== null &&
      itemData.next !== undefined;
      page++
    ) {
      if (signal.aborted || sequence !== loadSeq.current) return;
      const more = await gatewayRequest(
        "GET",
        api(`${itemPath}&before=${itemData.next}`),
        undefined,
        { signal },
      );
      itemData.items.push(...(more.items ?? []));
      itemData.next = more.next ?? null;
    }
    if (signal.aborted || sequence !== loadSeq.current) return;
    setFeeds((feedData.feeds as Feed[] | undefined) ?? []);
    setItems((itemData.items as FeedItem[] | undefined) ?? []);
    setNext(itemData.next ?? null);
    setScheduler((status.scheduler as SchedulerStatus | undefined) ?? null);
  }
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController();
    lifecycle.current = abort;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        if (!mutation.current) await fetchAll(abort.signal);
      } catch (failure) {
        if (!abort.signal.aborted)
          toast.error(String(failure), { id: "rss-request-error" });
      }
      if (!abort.signal.aborted) timer = setTimeout(() => void poll(), 5000);
    };
    void poll();
    void gatewayRequest("GET", "/bib/collections?view=summary", undefined, {
      signal: abort.signal,
    })
      .then((data) => {
        if (!abort.signal.aborted) setCollections(data.collections ?? []);
      })
      .catch(() => {});
    return () => {
      clearTimeout(timer);
      abort.abort();
      loadSeq.current++;
      lifecycle.current = null;
      setBusy(false);
    };
  }, [active]);
  async function act(
    work: (signal: AbortSignal) => Promise<void>,
    refresh = true,
    propagate = false,
  ) {
    const abort = lifecycle.current;
    if (!abort || mutation.current) {
      if (propagate) throw new Error(t("rss.errBusy"));
      return;
    }
    mutation.current = true;
    loadSeq.current++;
    setBusy(true);
    try {
      await work(abort.signal);
      if (refresh && !abort.signal.aborted) {
        try {
          await fetchAll(abort.signal);
        } catch (failure) {
          if (!abort.signal.aborted)
            toast.error(
              t("rss.refreshFailedToast", { message: String(failure) }),
              { id: "rss-request-error" },
            );
        }
      }
    } catch (failure) {
      if (propagate) throw failure;
      if (!abort.signal.aborted)
        toast.error(String(failure), { id: "rss-request-error" });
    } finally {
      mutation.current = false;
      if (!abort.signal.aborted) setBusy(false);
    }
  }
  const feedName = (id: string) =>
    feeds.find((f) => f.id === id)?.spec.name ?? t("rss.feedFallback");
  const visibleItems = items.filter(
    (item) =>
      (!unreadOnly || !item.read) &&
      `${item.title} ${item.authors.join(" ")} ${item.summary ?? ""}`
        .toLocaleLowerCase()
        .includes(query.toLocaleLowerCase()),
  );
  const selected = visibleItems.find((item) => item.id === selectedId);
  const unreadCount = feeds.reduce((sum, feed) => sum + feed.unread_count, 0);
  const selectFeed = (id: string) => {
    setShowFeeds(false);
    if (id === filterRef.current) return;
    loadedPages.current = 1;
    loadSeq.current++;
    setItems([]);
    setNext(null);
    setFilterFeed(id);
    setSelectedId(null);
    filterRef.current = id;
    const abort = lifecycle.current;
    if (abort)
      void fetchAll(abort.signal).catch((failure) => {
        if (!abort.signal.aborted)
          toast.error(String(failure), { id: "rss-request-error" });
      });
  };
  const importItem = (item: FeedItem) =>
    void act(async (signal) => {
      const destination = collections.find((c) => c.id === collection);
      await gatewayRequest(
        "POST",
        api(`/items${path(item.id)}`),
        destination ? { collection_id: destination.id } : {},
        { signal },
      );
      toast.success(
        t("rss.importedToast", {
          title: item.title,
          target: destination?.name ?? t("rss.library"),
        }),
      );
    });
  const sidebar = (
    <aside
      aria-label={t("rss.sidebarAria")}
      className={`rss-sidebar ${externalSidebar ? "rss-workbench-sidebar" : ""}`}
    >
      <div className="rss-sidebar-actions">
        <Button
          size="sm"
          variant="ghost"
          disabled={busy}
          onClick={() =>
            void act(async (signal) => {
              await fetchAll(signal);
              toast.success(t("rss.refreshedToast"));
            }, false)
          }
        >
          <RefreshCw className={busy ? "animate-spin" : ""} />
          {t("rss.refresh")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !feeds.length}
          onClick={() =>
            void act(async (signal) => {
              let newItems = 0;
              const failures: string[] = [];
              for (const feed of feeds.filter((f) => f.spec.enabled)) {
                const result = await gatewayRequest(
                  "POST",
                  api(feedPath(feed.id)),
                  undefined,
                  { signal },
                );
                const outcome = result.outcome as CheckOutcome;
                if (outcome.status === "error")
                  failures.push(`${feed.spec.name}: ${outcome.error}`);
                else newItems += outcome.new_items;
              }
              if (failures.length) toast.error(failures.join("; "));
              else
                toast.success(t("rss.feedsCheckedToast", { count: newItems }));
            })
          }
        >
          {t("rss.checkAll")}
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => setEditor({ initial: defaultSpec() })}
        >
          <Plus />
          {t("rss.addFeed")}
        </Button>
      </div>

      <button
        className={`rss-nav ${!filterFeed ? "is-active" : ""}`}
        onClick={() => selectFeed("")}
      >
        <Inbox size={16} />
        <span>{t("rss.allFeeds")}</span>
        <span className="rss-count">{unreadCount}</span>
      </button>
      <div className="rss-sidebar-heading">
        <h3>{t("rss.subscriptions")}</h3>
        <span>{feeds.length}</span>
      </div>
      {!feeds.length && <p className="text-xs">{t("rss.noFeedsYet")}</p>}
      {feeds.map((feed) => (
        <article
          key={feed.id}
          className={`rss-feed ${filterFeed === feed.id ? "is-active" : ""}`}
        >
          <div className="rss-feed-row">
            <button
              className="rss-feed-select"
              aria-pressed={filterFeed === feed.id}
              onClick={() => selectFeed(feed.id)}
            >
              <span className="rss-source-icon">
                {feed.spec.name.slice(0, 1).toUpperCase()}
              </span>
              <span className="rss-feed-name">{feed.spec.name}</span>
              <span className="rss-count">{feed.unread_count}</span>
            </button>
            <Button
              size="icon-sm"
              variant="ghost"
              disabled={busy}
              aria-label={t("rss.deleteAria", { name: feed.spec.name })}
              title={t("rss.deleteTitle")}
              onClick={() => {
                setDeleteError("");
                setConfirm(structuredClone(feed));
              }}
            >
              <Trash2 />
            </Button>
          </div>
          <details className="rss-feed-settings">
            <summary>
              {t("rss.manageSubscription")}
              {!feed.spec.enabled && ` · ${t("rss.paused")}`}
              {feed.last_error && ` · ${t("rss.errorWord")}`}
            </summary>
            <p className="break-words text-muted-foreground">{feed.spec.url}</p>
            <p>
              {feed.spec.enabled
                ? t("rss.automaticEvery", {
                    minutes: minutes(feed.spec.interval_seconds),
                  })
                : t("rss.paused")}{" "}
              ·{" "}
              {t("rss.itemsUnread", {
                items: feed.item_count,
                unread: feed.unread_count,
              })}
            </p>
            <p>
              {t("rss.lastNextCheck", {
                last: date(feed.last_success_ms),
                next: feed.spec.enabled
                  ? date(feed.next_due_ms)
                  : t("rss.paused"),
              })}
            </p>
            <div className="flex flex-wrap gap-1">
              <Button
                size="xs"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void act(async (signal) => {
                    const result = await gatewayRequest(
                      "POST",
                      api(feedPath(feed.id)),
                      undefined,
                      { signal },
                    );
                    const outcome = result.outcome as CheckOutcome;
                    if (outcome.status === "error")
                      toast.error(
                        t("rss.checkFailedToast", { error: outcome.error }),
                      );
                    else
                      toast.success(
                        outcome.status === "not_modified"
                          ? t("rss.noChangesToast")
                          : t("rss.newItemsToast", {
                              count: outcome.new_items,
                            }),
                      );
                  })
                }
              >
                {t("rss.checkNow")}
              </Button>
              <Button
                size="xs"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void act(async (signal) => {
                    await gatewayRequest(
                      "POST",
                      api(`${feedPath(feed.id)}/enabled`),
                      { version: feed.version, enabled: !feed.spec.enabled },
                      { signal },
                    );
                  })
                }
              >
                {feed.spec.enabled ? t("rss.pause") : t("rss.resume")}
              </Button>
              <Button
                size="xs"
                variant="ghost"
                disabled={busy}
                onClick={() =>
                  setEditor({
                    initial: structuredClone(feed.spec),
                    id: feed.id,
                    version: feed.version,
                  })
                }
              >
                {t("rss.edit")}
              </Button>
            </div>
          </details>
          {feed.last_error && (
            <div role="alert" className="rss-feed-error">
              <p>{feed.last_error}</p>
              <p>
                {feed.last_error.includes("404")
                  ? t("rss.error404Hint")
                  : t("rss.errorGenericHint")}
              </p>
            </div>
          )}
        </article>
      ))}
      <div className="rss-sync">
        <span
          className={`rss-sync-dot ${scheduler?.running ? "is-running" : ""}`}
        />
        <span>
          {scheduler
            ? scheduler.running
              ? scheduler.active_checks
                ? t("rss.checksActive", { count: scheduler.active_checks })
                : t("rss.syncOn")
              : t("rss.syncStopped")
            : t("rss.connecting")}
        </span>
      </div>
      {scheduler?.last_error && (
        <p role="alert" className="text-xs text-destructive">
          {t("rss.backgroundServiceError", { error: scheduler.last_error })}
        </p>
      )}
    </aside>
  );
  return (
    <section
      aria-label={t("rss.sectionAria")}
      className={`rss-workspace ${externalSidebar ? "rss-external-sidebar" : ""} ${showFeeds ? "rss-show-feeds" : ""}`}
    >
      {externalSidebar &&
        active &&
        sidebarSlot &&
        createPortal(sidebar, sidebarSlot)}
      <header className="rss-header">
        <div className="rss-brand">
          <Rss className="size-4 text-muted-foreground" />
          <div>
            <h2>{t("rss.subscriptions")}</h2>
          </div>
        </div>
        <div className="rss-header-actions">
          {externalSidebar ? (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={t("rss.showSidebarAria")}
              onClick={onOpenSidebar}
            >
              <PanelLeft />
            </Button>
          ) : (
            <Button
              className="rss-manage-toggle"
              size="sm"
              variant="outline"
              onClick={() => setShowFeeds((old) => !old)}
            >
              {t("rss.manageFeeds")}
            </Button>
          )}
        </div>
      </header>
      {editor && (
        <FeedEditor
          key={editor.id ?? "new"}
          initial={editor.initial}
          title={editor.id ? t("rss.editFeed") : t("rss.addFeed")}
          onClose={() => setEditor(null)}
          onSave={async (spec) => {
            await act(
              async (signal) => {
                if (editor.id && editor.version !== undefined)
                  await gatewayRequest(
                    "PUT",
                    api(feedPath(editor.id)),
                    { version: editor.version, spec },
                    { signal },
                  );
                else
                  await gatewayRequest("POST", api("/feeds"), spec, { signal });
                setEditor(null);
                toast.success(
                  editor.id
                    ? t("rss.feedUpdatedToast")
                    : t("rss.feedAddedToast"),
                );
              },
              true,
              true,
            );
          }}
        />
      )}
      {confirm && (
        <SearchDialog
          title={t("rss.confirmDeleteTitle")}
          onClose={() => {
            if (!busy) setConfirm(null);
          }}
        >
          <p className="text-xs">
            {t("rss.confirmDeleteBody", { name: confirm.spec.name })}
          </p>
          {deleteError && (
            <p role="alert" className="mt-2 text-xs text-destructive">
              {deleteError}
            </p>
          )}
          <div className="mt-3 flex justify-end gap-2">
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => setConfirm(null)}
            >
              {t("rss.keepFeed")}
            </Button>
            <Button
              size="sm"
              variant="destructive"
              disabled={busy}
              onClick={() => {
                setDeleteError("");
                void act(
                  async (signal) => {
                    await gatewayRequest(
                      "DELETE",
                      api(`${feedPath(confirm.id)}?version=${confirm.version}`),
                      undefined,
                      { signal },
                    );
                    setFeeds((old) =>
                      old.filter((feed) => feed.id !== confirm.id),
                    );
                    setItems((old) =>
                      old.filter((item) => item.feed_id !== confirm.id),
                    );
                    if (filterRef.current === confirm.id) selectFeed("");
                    setConfirm(null);
                    toast.success(t("rss.feedDeletedToast"));
                  },
                  true,
                  true,
                ).catch((failure) => setDeleteError(String(failure)));
              }}
            >
              {busy ? t("rss.deleting") : t("rss.confirmDeletion")}
            </Button>
          </div>
        </SearchDialog>
      )}

      <div className={`rss-columns ${selected ? "rss-has-selection" : ""}`}>
        {!externalSidebar && sidebar}

        <section aria-label={t("rss.inboxAria")} className="rss-timeline">
          <div className="rss-timeline-header">
            <div>
              <h3>
                {filterFeed ? feedName(filterFeed) : t("rss.allArticles")}
              </h3>
              <p className="rss-subtitle">
                {t("rss.inboxSubtitle", {
                  loaded: visibleItems.length,
                  unread: unreadCount,
                })}
              </p>
            </div>
            <label className="rss-search">
              <Search size={15} />
              <Input
                aria-label={t("rss.searchArticlesAria")}
                placeholder={t("rss.searchArticlesPlaceholder")}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </label>
          </div>
          <div className="rss-toolbar">
            <div className="rss-tabs" aria-label={t("rss.articleStatusAria")}>
              <button
                aria-pressed={!unreadOnly}
                className={!unreadOnly ? "is-active" : ""}
                onClick={() => setUnreadOnly(false)}
              >
                {t("rss.allArticles")}
              </button>
              <button
                aria-pressed={unreadOnly}
                className={unreadOnly ? "is-active" : ""}
                onClick={() => setUnreadOnly(true)}
              >
                {t("rss.unread")}
              </button>
            </div>
            <label className="rss-source-filter">
              {t("rss.source")}
              <select
                aria-label={t("rss.sourceFilterAria")}
                value={filterFeed}
                onChange={(e) => selectFeed(e.target.value)}
              >
                <option value="">{t("rss.allFeeds")}</option>
                {feeds.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.spec.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="rss-article-list">
            {!visibleItems.length && (
              <div className="rss-empty">
                <span className="rss-empty-icon">
                  <Inbox size={30} strokeWidth={1.3} />
                </span>
                <h4>
                  {!feeds.length
                    ? t("rss.emptyNoSubscriptions")
                    : t("rss.emptyCaughtUp")}
                </h4>
                <p>
                  {!feeds.length
                    ? t("rss.emptyNoSubscriptionsBody")
                    : t("rss.emptyFilteredBody")}
                </p>
                {!feeds.length && (
                  <Button
                    size="sm"
                    disabled={busy}
                    onClick={() => setEditor({ initial: defaultSpec() })}
                  >
                    <Plus />
                    {t("rss.addFirstFeed")}
                  </Button>
                )}
              </div>
            )}
            {visibleItems.map((item) => (
              <article
                key={item.id}
                className={`rss-article ${item.read ? "is-read" : ""} ${selectedId === item.id ? "is-selected" : ""}`}
              >
                <button
                  className="rss-article-select"
                  onClick={() => setSelectedId(item.id)}
                  aria-pressed={selectedId === item.id}
                >
                  <span className="rss-article-meta">
                    <span className="rss-mini-source">
                      <Rss size={11} />
                      {feedName(item.feed_id)}
                    </span>
                    <span>{date(item.published_ms ?? item.first_seen_ms)}</span>
                    {!item.read && (
                      <span
                        className="rss-unread-dot"
                        title={t("rss.unread")}
                      />
                    )}
                  </span>
                  <h4>{item.title}</h4>
                  {item.summary && (
                    <p className="rss-excerpt">{plainSummary(item.summary)}</p>
                  )}
                  {item.authors.length > 0 && (
                    <span
                      className="rss-authors"
                      title={item.authors.join(", ")}
                    >
                      {item.authors.join(", ")}
                    </span>
                  )}
                </button>
                <div className="rss-article-actions">
                  {item.imported_article_id ? (
                    <span className="rss-imported">
                      <Check size={12} />
                      {t("rss.imported")}
                    </span>
                  ) : (
                    <Button
                      size="xs"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => importItem(item)}
                    >
                      <Library />
                      {t("rss.import")}
                    </Button>
                  )}
                  <Button
                    size="xs"
                    variant="ghost"
                    disabled={busy}
                    onClick={() =>
                      void act(async (signal) => {
                        await gatewayRequest(
                          "PUT",
                          api(`/items${path(item.id)}`),
                          { read: !item.read },
                          { signal },
                        );
                      })
                    }
                  >
                    {item.read ? t("rss.markUnread") : t("rss.markRead")}
                  </Button>
                </div>
              </article>
            ))}
            {next !== null && (
              <Button
                className="rss-load-more"
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void act(async (signal) => {
                    const source = filterRef.current;
                    const sequence = loadSeq.current;
                    const base = filterFeed
                      ? `/items?feed=${encodeURIComponent(filterFeed)}&`
                      : "/items?";
                    const data = await gatewayRequest(
                      "GET",
                      api(`${base}before=${next}&limit=50`),
                      undefined,
                      { signal },
                    );
                    if (
                      signal.aborted ||
                      sequence !== loadSeq.current ||
                      source !== filterRef.current
                    )
                      return;
                    loadedPages.current++;
                    setItems((old) =>
                      Array.from(
                        new Map(
                          [...old, ...(data.items as FeedItem[])].map(
                            (item) => [item.id, item],
                          ),
                        ).values(),
                      ),
                    );
                    setNext(data.next ?? null);
                  }, false)
                }
              >
                {t("rss.loadMore")}
              </Button>
            )}
          </div>
          <footer className="rss-destination">
            <Library size={14} />
            <label>
              {t("rss.importDestination")}
              <select
                aria-label={t("rss.importDestinationAria")}
                value={collection}
                onChange={(e) => setCollection(e.target.value)}
              >
                <option value="">{t("rss.library")}</option>
                {collections
                  .filter((c) => !c.deleted)
                  .map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
              </select>
            </label>
          </footer>
        </section>
        <section aria-label={t("rss.previewAria")} className="rss-reader">
          {selected ? (
            <>
              <div className="rss-reader-toolbar">
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setSelectedId(null)}
                >
                  <ArrowLeft />
                  {t("rss.backToArticles")}
                </Button>
                {selected.link && safeLink(selected.link) && (
                  <a href={selected.link} target="_blank" rel="noreferrer">
                    {t("rss.openOriginal")}
                    <ArrowUpRight size={14} />
                  </a>
                )}
              </div>
              <div className="rss-reader-content">
                <p className="rss-eyebrow">{feedName(selected.feed_id)}</p>
                <h3>{selected.title}</h3>
                <p className="rss-subtitle">
                  {date(selected.published_ms ?? selected.first_seen_ms)}
                </p>
                {selected.authors.length > 0 && (
                  <p className="rss-reader-authors">
                    {selected.authors.join(", ")}
                  </p>
                )}
                <hr />
                <p className="rss-reader-summary">
                  {selected.summary
                    ? plainSummary(selected.summary)
                    : t("rss.noSummary")}
                </p>
                <div className="rss-reader-bottom">
                  {selected.imported_article_id ? (
                    <span className="rss-imported">
                      <Check size={14} />
                      {t("rss.importedToLibrary")}
                    </span>
                  ) : (
                    <Button
                      size="sm"
                      disabled={busy}
                      onClick={() => importItem(selected)}
                    >
                      <Library />
                      {t("rss.importTo", {
                        target: collection
                          ? (collections.find((c) => c.id === collection)
                              ?.name ?? t("rss.collectionWord"))
                          : t("rss.library"),
                      })}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    onClick={() =>
                      void act(async (signal) => {
                        await gatewayRequest(
                          "DELETE",
                          api(`/items${path(selected.id)}`),
                          undefined,
                          { signal },
                        );
                        setSelectedId(null);
                      })
                    }
                  >
                    {t("rss.delete")}
                  </Button>
                </div>
              </div>
            </>
          ) : (
            <div className="rss-empty">
              <span className="rss-empty-icon">
                <Rss size={30} strokeWidth={1.3} />
              </span>
              <h4>{t("rss.selectArticle")}</h4>
              <p>{t("rss.selectArticleBody")}</p>
            </div>
          )}
        </section>
      </div>
    </section>
  );
}

function plainSummary(summary: string) {
  const document = new DOMParser().parseFromString(summary, "text/html");
  return document.body.textContent?.trim() ?? "";
}
function safeLink(link: string) {
  try {
    return ["https:", "http:"].includes(new URL(link).protocol);
  } catch {
    return false;
  }
}
