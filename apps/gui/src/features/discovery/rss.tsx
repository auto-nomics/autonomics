import { useEffect, useRef, useState } from "react";
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

export function RssSubscriptions({ active = true }: { active?: boolean }) {
  const [feeds, setFeeds] = useState<Feed[]>([]);
  const [items, setItems] = useState<FeedItem[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [scheduler, setScheduler] = useState<SchedulerStatus | null>(null);
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [collection, setCollection] = useState("");
  const [filterFeed, setFilterFeed] = useState("");
  const [editor, setEditor] = useState<{ initial: FeedSpec; id?: string; version?: number } | null>(null);
  const [confirm, setConfirm] = useState<Feed | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
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
    setFeeds(feedData.feeds as Feed[]);
    setItems(itemData.items as FeedItem[]);
    setNext(itemData.next);
    setScheduler(status.scheduler as SchedulerStatus);
  }
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController(); lifecycle.current = abort; let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { if (!mutation.current) await fetchAll(abort.signal); } catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
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
    mutation.current = true; setBusy(true); setError("");
    try { await work(abort.signal); if (!abort.signal.aborted) await fetchAll(abort.signal); }
    catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
    finally { mutation.current = false; if (!abort.signal.aborted) setBusy(false); }
  }
  const feedName = (id: string) => feeds.find(f => f.id === id)?.spec.name ?? "Feed";
  return <section aria-label="RSS subscriptions" className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3 text-sm">
    <header className="flex flex-wrap items-center justify-between gap-2">
      <h2 className="font-medium">Subscriptions</h2>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" onClick={() => setEditor({ initial: defaultSpec() })}>Add feed</Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => { await fetchAll(signal); })}>Refresh</Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => { for (const feed of feeds.filter(f => f.spec.enabled)) await gatewayRequest("POST", api(`${path(feed.id)}/check`), undefined, { signal }); })}>Check all</Button>
      </div>
    </header>
    <p className="text-xs text-muted-foreground">Subscribe to RSS/Atom feeds (journals, arXiv categories, search alerts). Entries are fetched on a schedule and reviewed here before import.</p>
    {scheduler && <p className="text-xs">{scheduler.running ? `${scheduler.active_checks} check(s) active` : "Background service stopped"}{scheduler.last_error && <span> · Last service error: {scheduler.last_error}</span>}</p>}
    {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    {notice && <p role="status" className="text-xs">{notice}</p>}
    {editor && <FeedEditor key={editor.id ?? "new"} initial={editor.initial} title={editor.id ? "Edit feed" : "Add feed"} onClose={() => setEditor(null)} onSave={async spec => {
      if (editor.id && editor.version !== undefined) await gatewayRequest("PUT", api(path(editor.id)), { version: editor.version, spec });
      else await gatewayRequest("POST", api("/feeds"), spec);
      setEditor(null);
    }} />}
    {confirm && <SearchDialog title="Confirm feed deletion" onClose={() => setConfirm(null)}><p className="text-xs">Delete “{confirm.spec.name}”, its inbox items and scheduling state? Imported library papers are kept.</p><div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={() => setConfirm(null)}>Keep feed</Button><Button size="sm" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("DELETE", api(`${path(confirm.id)}?version=${confirm.version}`), undefined, { signal }); setConfirm(null); })}>Confirm deletion</Button></div></SearchDialog>}

    <section aria-label="Feed list" className="space-y-2"><h3 className="font-medium">Feeds</h3>
      {!feeds.length && <p className="text-xs">No feeds yet.</p>}
      {feeds.map(feed => <article key={feed.id} className="space-y-2 rounded border p-2 text-xs">
        <p className="break-words font-medium">{feed.spec.name}</p>
        <p className="break-words text-muted-foreground">{feed.spec.url}</p>
        <p>{feed.spec.enabled ? `Automatic checks every ${minutes(feed.spec.interval_seconds)} min` : "Paused"} · {feed.item_count} items ({feed.unread_count} unread)</p>
        <p>Last successful check: {date(feed.last_success_ms)} · Next scheduled check: {feed.spec.enabled ? date(feed.next_due_ms) : "Paused"}</p>
        {feed.last_error && <p role="alert" className="text-destructive">{feed.last_error}</p>}
        <div className="flex flex-wrap gap-1">
          <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => {
            const result = await gatewayRequest("POST", api(`${path(feed.id)}/check`), undefined, { signal });
            const outcome = result.outcome as CheckOutcome;
            setNotice(outcome.status === "error" ? `Check failed: ${outcome.error}` : outcome.status === "not_modified" ? `No changes since the last check.` : `${outcome.new_items} new item(s).`);
          })}>Check now</Button>
          <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("POST", api(`${path(feed.id)}/enabled`), { version: feed.version, enabled: !feed.spec.enabled }, { signal }); })}>{feed.spec.enabled ? "Pause" : "Resume"}</Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => setEditor({ initial: structuredClone(feed.spec), id: feed.id, version: feed.version })}>Edit</Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirm(structuredClone(feed))}>Delete</Button>
        </div>
      </article>)}
    </section>

    <section aria-label="Feed inbox" className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-medium">Inbox</h3>
        <div className="flex items-center gap-2 text-xs">
          <label>Show<select aria-label="Inbox feed filter" className="rounded border bg-background p-1" value={filterFeed} onChange={e => { setFilterFeed(e.target.value); if (lifecycle.current) void act(async signal => { const data = await gatewayRequest("GET", api(e.target.value ? `/items?feed=${encodeURIComponent(e.target.value)}&limit=50` : "/items?limit=50"), undefined, { signal }); setItems(data.items); setNext(data.next); }); }}><option value="">All feeds</option>{feeds.map(f => <option key={f.id} value={f.id}>{f.spec.name}</option>)}</select></label>
          <label>Import destination<select aria-label="RSS import destination" className="rounded border bg-background p-1" value={collection} onChange={e => setCollection(e.target.value)}><option value="">Library</option>{collections.filter(c => !c.deleted).map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
        </div>
      </div>
      {!items.length && <p className="text-xs">No inbox items loaded.</p>}
      {items.map(item => <article key={item.id} className="space-y-2 rounded border p-2 text-xs">
        <p className="break-words font-medium">{item.title}</p>
        <p>{feedName(item.feed_id)} · {item.read ? "Read" : "Unread"} · published {date(item.published_ms)} · first seen {date(item.first_seen_ms)}</p>
        {item.authors.length > 0 && <p className="text-muted-foreground">{item.authors.join(", ")}</p>}
        {item.summary && <p className="break-words text-muted-foreground">{item.summary.slice(0, 400)}{item.summary.length > 400 && "…"}</p>}
        {item.link && <a href={item.link} target="_blank" rel="noreferrer" className="break-words underline">{item.link}</a>}
        <div className="flex flex-wrap gap-1">
          {item.imported_article_id
            ? <span role="status">Imported</span>
            : <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => {
                const destination = collections.find(c => c.id === collection);
                await gatewayRequest("POST", api(`/items${path(item.id)}`), destination ? { collection_id: destination.id } : {}, { signal });
                setNotice(`Imported “${item.title}” to ${destination?.name ?? "Library"}.`);
              })}>Import to {collection ? collections.find(c => c.id === collection)?.name ?? "collection" : "Library"}</Button>}
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("PUT", api(`/items${path(item.id)}`), { read: !item.read }, { signal }); })}>{item.read ? "Mark unread" : "Mark read"}</Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("DELETE", api(`/items${path(item.id)}`), undefined, { signal }); })}>Delete</Button>
        </div>
      </article>)}
      {next !== null && <Button size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => {
        const base = filterFeed ? `/items?feed=${encodeURIComponent(filterFeed)}&` : "/items?";
        const data = await gatewayRequest("GET", api(`${base}before=${next}&limit=50`), undefined, { signal });
        setItems(old => [...old, ...(data.items as FeedItem[])]); setNext(data.next);
      })}>Load more</Button>}
    </section>
  </section>;
}
