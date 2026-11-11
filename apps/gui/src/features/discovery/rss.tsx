import { ArrowLeft, ArrowUpRight, Check, Inbox, Library, Plus, PanelLeft, RefreshCw, Rss, Search } from "lucide-react";
import "./rss.css";
import { useEffect, useRef, useState } from "react";
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
export interface SchedulerStatus { running: boolean; active_checks: number; last_error: string | null }

const api = (suffix = "") => `/bib/rss${suffix}`;
const path = (id: string) => `/${encodeURIComponent(id)}`;
const date = (ms: number | null) => (ms === null ? "Never" : new Date(ms).toLocaleString());
const minutes = (seconds: number) => Math.round(seconds / 60);
const defaultSpec = (): FeedSpec => ({ name: "", url: "", enabled: true, interval_seconds: 3600, max_items_per_feed: 500 });

function FeedEditor({ initial, title, onClose, onSave }: { initial: FeedSpec; title: string; onClose: () => void; onSave: (spec: FeedSpec) => Promise<void> }) {
  const [draft, setDraft] = useState(() => structuredClone(initial));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const update = <K extends keyof FeedSpec>(key: K, value: FeedSpec[K]) => setDraft(old => ({ ...old, [key]: value }));
  async function save() {
    setBusy(true); setError("");
    try {
      const name = draft.name.trim();
      const url = draft.url.trim();
      if (!name || !/^https?:\/\//.test(url)) throw new Error("A name and an absolute http(s) feed URL are required.");
      if (draft.interval_seconds < 300 || draft.interval_seconds > 2_592_000) throw new Error("Check interval must be 5 minutes – 30 days.");
      if (draft.max_items_per_feed < 1 || draft.max_items_per_feed > 5000) throw new Error("Item retention must be 1–5000.");
      await onSave({ ...draft, name, url });
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  }
  return <SearchDialog title={title} onClose={onClose} footer={<div className="flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button><Button size="sm" disabled={busy} onClick={() => void save()}>{busy ? "Saving…" : "Save feed"}</Button></div>}>
    <fieldset disabled={busy} className="space-y-3">
      <label className="block text-xs">Feed name<Input aria-label="Feed name" value={draft.name} maxLength={256} onChange={e => update("name", e.target.value)} /></label>
      <label className="block text-xs">Feed URL (RSS 2.0 or Atom)<Input aria-label="Feed URL" placeholder="https://…" value={draft.url} onChange={e => update("url", e.target.value)} /></label>
      <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={draft.enabled} onChange={e => update("enabled", e.target.checked)} />Enable automatic checks</label>
      <div className="grid grid-cols-2 gap-3 text-xs">
        <label>Check interval (minutes)<Input aria-label="Check interval (minutes)" type="number" min={5} max={43200} value={minutes(draft.interval_seconds)} onChange={e => update("interval_seconds", Number(e.target.value) * 60)} /></label>
        <label>Retained items per feed<Input aria-label="Retained items per feed" type="number" min={1} max={5000} value={draft.max_items_per_feed} onChange={e => update("max_items_per_feed", Number(e.target.value))} /></label>
      </div>
      <p className="text-xs text-muted-foreground">Checks run in the background while this view is closed. New entries stay in the local inbox until you import them.</p>
    </fieldset>
    {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
  </SearchDialog>;
}

export function RssSubscriptions({ active = true, sidebarSlot, externalSidebar = false, onOpenSidebar }: {
  active?: boolean;
  sidebarSlot?: HTMLElement | null;
  externalSidebar?: boolean;
  onOpenSidebar?: () => void;
}) {
  const [feeds, setFeeds] = useState<Feed[]>([]);
  const [items, setItems] = useState<FeedItem[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [scheduler, setScheduler] = useState<SchedulerStatus | null>(null);
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [collection, setCollection] = useState("");
  const [filterFeed, setFilterFeed] = useState("");
  const [editor, setEditor] = useState<{ initial: FeedSpec; id?: string; version?: number } | null>(null);
  const [confirm, setConfirm] = useState<Feed | null>(null);
  const [busy, setBusy] = useState(false);
  const [showFeeds, setShowFeeds] = useState(false);
  const [query, setQuery] = useState("");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const lifecycle = useRef<AbortController | null>(null);
  const mutation = useRef(false);
  const loadSeq = useRef(0);
  const filterRef = useRef(""); filterRef.current = filterFeed;

  async function fetchAll(signal: AbortSignal) {
    const sequence = ++loadSeq.current;
    const itemPath = filterRef.current ? `/items?feed=${encodeURIComponent(filterRef.current)}&limit=50` : "/items?limit=50";
    const [feedData, itemData, status] = await Promise.all([
      gatewayRequest("GET", api("/feeds"), undefined, { signal }),
      gatewayRequest("GET", api(itemPath), undefined, { signal }),
      gatewayRequest("GET", api("/status"), undefined, { signal }),
    ]);
    if (signal.aborted || sequence !== loadSeq.current) return;
    setFeeds((feedData.feeds as Feed[] | undefined) ?? []);
    setItems((itemData.items as FeedItem[] | undefined) ?? []);
    setNext(itemData.next ?? null);
    setScheduler((status.scheduler as SchedulerStatus | undefined) ?? null);
  }
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController(); lifecycle.current = abort; let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { if (!mutation.current) await fetchAll(abort.signal); } catch (failure) { if (!abort.signal.aborted) toast.error(String(failure), { id: "rss-request-error" }); }
      if (!abort.signal.aborted) timer = setTimeout(() => void poll(), 5000);
    };
    void poll();
    void gatewayRequest("GET", "/bib/collections?view=summary", undefined, { signal: abort.signal })
      .then(data => { if (!abort.signal.aborted) setCollections(data.collections ?? []); })
      .catch(() => {});
    return () => { clearTimeout(timer); abort.abort(); loadSeq.current++; lifecycle.current = null; setBusy(false); };
  }, [active]);
  async function act(work: (signal: AbortSignal) => Promise<void>) {
    const abort = lifecycle.current; if (!abort || mutation.current) return;
    mutation.current = true; setBusy(true);
    try { await work(abort.signal); if (!abort.signal.aborted) await fetchAll(abort.signal); }
    catch (failure) { if (!abort.signal.aborted) toast.error(String(failure), { id: "rss-request-error" }); }
    finally { mutation.current = false; if (!abort.signal.aborted) setBusy(false); }
  }
  const feedName = (id: string) => feeds.find(f => f.id === id)?.spec.name ?? "Feed";
  const visibleItems = items.filter(item => (!unreadOnly || !item.read) &&
    `${item.title} ${item.authors.join(" ")} ${item.summary ?? ""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  const selected = visibleItems.find(item => item.id === selectedId);
  const unreadCount = feeds.reduce((sum, feed) => sum + feed.unread_count, 0);
  const selectFeed = (id: string) => { setShowFeeds(false); setFilterFeed(id); filterRef.current = id; setSelectedId(null); if (lifecycle.current) void act(fetchAll); };
  const importItem = (item: FeedItem) => void act(async signal => {
    const destination = collections.find(c => c.id === collection);
    await gatewayRequest("POST", api(`/items${path(item.id)}`), destination ? { collection_id: destination.id } : {}, { signal });
    toast.success(`Imported “${item.title}” to ${destination?.name ?? "Library"}.`);
  });
  const sidebar = (
    <aside aria-label="Feed list" className={`rss-sidebar ${externalSidebar ? "rss-workbench-sidebar" : ""}`}>
      <div className="rss-sidebar-actions">
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await fetchAll(signal); toast.success("Subscriptions refreshed."); })}><RefreshCw className={busy ? "animate-spin" : ""} />Refresh</Button>
        <Button size="sm" variant="outline" disabled={busy || !feeds.length} onClick={() => void act(async signal => { let newItems = 0;
          const failures: string[] = [];
          for (const feed of feeds.filter(f => f.spec.enabled)) {
            const result = await gatewayRequest("POST", api(`${path(feed.id)}/check`), undefined, { signal });
            const outcome = result.outcome as CheckOutcome;
            if (outcome.status === "error") failures.push(`${feed.spec.name}: ${outcome.error}`);
            else newItems += outcome.new_items;
          }
          if (failures.length) toast.error(failures.join("; "));
          else toast.success(`Feeds checked · ${newItems} new item(s).`); })}>Check all</Button>
        <Button size="sm" onClick={() => setEditor({ initial: defaultSpec() })}><Plus />Add feed</Button>
      </div>

      <button className={`rss-nav ${!filterFeed ? "is-active" : ""}`} onClick={() => selectFeed("")}><Inbox size={16} /><span>All feeds</span><span className="rss-count">{unreadCount}</span></button>
      <div className="rss-sidebar-heading"><h3>Subscriptions</h3><span>{feeds.length}</span></div>
      {!feeds.length && <p className="text-xs">No feeds yet.</p>}
      {feeds.map(feed => <article key={feed.id} className={`rss-feed ${filterFeed === feed.id ? "is-active" : ""}`}>
        <button className="rss-feed-select" aria-pressed={filterFeed === feed.id} onClick={() => selectFeed(feed.id)}><span className="rss-source-icon">{feed.spec.name.slice(0, 1).toUpperCase()}</span><span className="rss-feed-name">{feed.spec.name}</span><span className="rss-count">{feed.unread_count}</span></button>
        <details className="rss-feed-settings"><summary>Manage subscription{!feed.spec.enabled && " · Paused"}{feed.last_error && " · Error"}</summary>
        <p className="break-words text-muted-foreground">{feed.spec.url}</p>
        <p>{feed.spec.enabled ? `Automatic checks every ${minutes(feed.spec.interval_seconds)} min` : "Paused"} · {feed.item_count} items ({feed.unread_count} unread)</p>
        <p>Last successful check: {date(feed.last_success_ms)} · Next scheduled check: {feed.spec.enabled ? date(feed.next_due_ms) : "Paused"}</p>
        {feed.last_error && <p role="alert" className="text-destructive">{feed.last_error}</p>}
        <div className="flex flex-wrap gap-1">
          <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => {
            const result = await gatewayRequest("POST", api(`${path(feed.id)}/check`), undefined, { signal });
            const outcome = result.outcome as CheckOutcome;
            if (outcome.status === "error") toast.error(`Check failed: ${outcome.error}`);
            else toast.success(outcome.status === "not_modified" ? "No changes since the last check." : `${outcome.new_items} new item(s).`);
          })}>Check now</Button>
          <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("POST", api(`${path(feed.id)}/enabled`), { version: feed.version, enabled: !feed.spec.enabled }, { signal }); })}>{feed.spec.enabled ? "Pause" : "Resume"}</Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => setEditor({ initial: structuredClone(feed.spec), id: feed.id, version: feed.version })}>Edit</Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirm(structuredClone(feed))}>Delete</Button>
        </div>
      </details></article>)}
      <div className="rss-sync"><span className={`rss-sync-dot ${scheduler?.running ? "is-running" : ""}`} /><span>{scheduler ? scheduler.running ? scheduler.active_checks ? `${scheduler.active_checks} check(s) active` : "Background sync on" : "Background service stopped" : "Connecting…"}</span></div>
      {scheduler?.last_error && <p role="alert" className="text-xs text-destructive">{scheduler.last_error}</p>}
    </aside>
  );
  return <section aria-label="RSS subscriptions" className={`rss-workspace ${externalSidebar ? "rss-external-sidebar" : ""} ${showFeeds ? "rss-show-feeds" : ""}`}>
    {externalSidebar && active && sidebarSlot && createPortal(sidebar, sidebarSlot)}
    <header className="rss-header">
      <div className="rss-brand"><Rss className="size-4 text-muted-foreground" /><div><h2>Subscriptions</h2></div></div>
      <div className="rss-header-actions">{externalSidebar ? <Button size="icon-sm" variant="ghost" aria-label="Show subscriptions sidebar" onClick={onOpenSidebar}><PanelLeft /></Button> : <Button className="rss-manage-toggle" size="sm" variant="outline" onClick={() => setShowFeeds(old => !old)}>Manage feeds</Button>}
      </div>
    </header>
    {editor && <FeedEditor key={editor.id ?? "new"} initial={editor.initial} title={editor.id ? "Edit feed" : "Add feed"} onClose={() => setEditor(null)} onSave={async spec => {
      if (editor.id && editor.version !== undefined) await gatewayRequest("PUT", api(path(editor.id)), { version: editor.version, spec });
      else await gatewayRequest("POST", api("/feeds"), spec);
      setEditor(null);
      toast.success(editor.id ? "Feed updated." : "Feed added.");
      if (lifecycle.current) await fetchAll(lifecycle.current.signal);
    }} />}
    {confirm && <SearchDialog title="Confirm feed deletion" onClose={() => setConfirm(null)}><p className="text-xs">Delete “{confirm.spec.name}”, its inbox items and scheduling state? Imported library papers are kept.</p><div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={() => setConfirm(null)}>Keep feed</Button><Button size="sm" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("DELETE", api(`${path(confirm.id)}?version=${confirm.version}`), undefined, { signal }); setConfirm(null); })}>Confirm deletion</Button></div></SearchDialog>}

    <div className={`rss-columns ${selected ? "rss-has-selection" : ""}`}>
    {!externalSidebar && sidebar}


    <section aria-label="Feed inbox" className="rss-timeline">
      <div className="rss-timeline-header"><div><h3>{filterFeed ? feedName(filterFeed) : "All articles"}</h3><p className="rss-subtitle">{visibleItems.length} loaded articles · {unreadCount} unread across your feeds</p></div>
        <label className="rss-search"><Search size={15} /><Input aria-label="Search loaded articles" placeholder="Search loaded articles…" value={query} onChange={e => setQuery(e.target.value)} /></label>
      </div>
      <div className="rss-toolbar"><div className="rss-tabs" aria-label="Article status"><button aria-pressed={!unreadOnly} className={!unreadOnly ? "is-active" : ""} onClick={() => setUnreadOnly(false)}>All articles</button><button aria-pressed={unreadOnly} className={unreadOnly ? "is-active" : ""} onClick={() => setUnreadOnly(true)}>Unread</button></div>
        <label className="rss-source-filter">Source<select aria-label="Inbox feed filter" value={filterFeed} onChange={e => selectFeed(e.target.value)}><option value="">All feeds</option>{feeds.map(f => <option key={f.id} value={f.id}>{f.spec.name}</option>)}</select></label>
      </div>
      <div className="rss-article-list">
      {!visibleItems.length && <div className="rss-empty"><span className="rss-empty-icon"><Inbox size={30} strokeWidth={1.3} /></span><h4>{!feeds.length ? "No subscriptions yet." : "You're all caught up."}</h4><p>{!feeds.length ? "Follow journals, arXiv categories, or search alerts. New research will arrive here." : "No loaded articles match this view. Try another filter or check your feeds."}</p>{!feeds.length && <Button size="sm" onClick={() => setEditor({ initial: defaultSpec() })}><Plus />Add your first feed</Button>}</div>}
      {visibleItems.map(item => <article key={item.id} className={`rss-article ${item.read ? "is-read" : ""} ${selectedId === item.id ? "is-selected" : ""}`}>
        <button className="rss-article-select" onClick={() => setSelectedId(item.id)} aria-pressed={selectedId === item.id}>
          <span className="rss-article-meta"><span className="rss-mini-source"><Rss size={11} />{feedName(item.feed_id)}</span><span>{date(item.published_ms ?? item.first_seen_ms)}</span>{!item.read && <span className="rss-unread-dot" title="Unread" />}</span>
          <h4>{item.title}</h4>
          {item.summary && <p className="rss-excerpt">{plainSummary(item.summary)}</p>}
          {item.authors.length > 0 && <span className="rss-authors" title={item.authors.join(", ")}>{item.authors.join(", ")}</span>}
        </button>
        <div className="rss-article-actions">
          {item.imported_article_id ? <span className="rss-imported"><Check size={12} />Imported</span> : <Button size="xs" variant="ghost" disabled={busy} onClick={() => importItem(item)}><Library />Import</Button>}
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("PUT", api(`/items${path(item.id)}`), { read: !item.read }, { signal }); })}>{item.read ? "Mark unread" : "Mark read"}</Button>
        </div>
      </article>)}
      {next !== null && <Button className="rss-load-more" size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => {
        const base = filterFeed ? `/items?feed=${encodeURIComponent(filterFeed)}&` : "/items?";
        const data = await gatewayRequest("GET", api(`${base}before=${next}&limit=50`), undefined, { signal });
        setItems(old => [...old, ...(data.items as FeedItem[])]); setNext(data.next);
      })}>Load more</Button>}
      </div>
      <footer className="rss-destination"><Library size={14} /><label>Import destination<select aria-label="RSS import destination" value={collection} onChange={e => setCollection(e.target.value)}><option value="">Library</option>{collections.filter(c => !c.deleted).map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label></footer>
    </section>
    <section aria-label="Article preview" className="rss-reader">
      {selected ? <><div className="rss-reader-toolbar"><Button size="sm" variant="ghost" onClick={() => setSelectedId(null)}><ArrowLeft />Back to articles</Button>{selected.link && safeLink(selected.link) && <a href={selected.link} target="_blank" rel="noreferrer">Open original<ArrowUpRight size={14} /></a>}</div>
      <div className="rss-reader-content"><p className="rss-eyebrow">{feedName(selected.feed_id)}</p><h3>{selected.title}</h3><p className="rss-subtitle">{date(selected.published_ms ?? selected.first_seen_ms)}</p>{selected.authors.length > 0 && <p className="rss-reader-authors">{selected.authors.join(", ")}</p>}<hr /><p className="rss-reader-summary">{selected.summary ? plainSummary(selected.summary) : "This feed does not include a summary. Open the original article to keep reading."}</p><div className="rss-reader-bottom">{selected.imported_article_id ? <span className="rss-imported"><Check size={14} />Imported to library</span> : <Button size="sm" disabled={busy} onClick={() => importItem(selected)}><Library />Import to {collection ? collections.find(c => c.id === collection)?.name ?? "collection" : "Library"}</Button>}<Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("DELETE", api(`/items${path(selected.id)}`), undefined, { signal }); setSelectedId(null); })}>Delete</Button></div></div></> : <div className="rss-empty"><span className="rss-empty-icon"><Rss size={30} strokeWidth={1.3} /></span><h4>Select an article</h4><p>Select an article to read its summary and add it to your research library.</p></div>}
    </section>
    </div>
  </section>;
}

function plainSummary(summary: string) {
  const document = new DOMParser().parseFromString(summary, "text/html");
  return document.body.textContent?.trim() ?? "";
}
function safeLink(link: string) {
  try { return ["https:", "http:"].includes(new URL(link).protocol); } catch { return false; }
}
