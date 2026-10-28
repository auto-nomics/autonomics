import { useEffect, useRef, useState } from "react";
import { gatewayRequest, newGatewayRequestId } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { BibCollection } from "@/types";
import type { DiscoveryRequest } from "./discovery-model";
import { SearchDialog } from "@/features/bibliography";
import { IdentityImportReview, type ExplorationImportTarget } from "./exploration-import";
import { defaultSubscription, notificationLookup, type InboxItem, type NotificationLookup, type Run, type RunSummary, type SubscriptionLimits, type SubscriptionRule, type SubscriptionSpec } from "./subscription-model";
const sources = ["pubmed", "openalex", "crossref", "arxiv", "semantic_scholar"];
const date = (ms: number | null) => ms === null ? "Never" : new Date(ms).toLocaleString();
const api = (suffix = "") => `/bib/subscriptions${suffix}`;
const path = (id: string) => `/${encodeURIComponent(id)}`;
const pretty = (value: unknown) => JSON.stringify(value, null, 2);
const strategies = "PubMed checks complete UTC days for creation/revision, so today is checked later. Crossref uses index-date windows when supported. Other sources replay a bounded result range; this is not a complete update feed.";

function RuleEditor({ initial, title, onClose, onSave }: { initial: SubscriptionSpec; title: string; onClose: () => void; onSave: (spec: SubscriptionSpec) => Promise<void> }) {
  const [draft, setDraft] = useState(() => structuredClone(initial));
  const [nativeText, setNativeText] = useState(() => pretty(initial.request.native?.parameters ?? {}));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const update = <K extends keyof SubscriptionSpec>(key: K, value: SubscriptionSpec[K]) => setDraft(old => ({ ...old, [key]: value }));
  const search = (values: Partial<DiscoveryRequest>) => setDraft(old => ({ ...old, request: { ...old.request, ...values } }));
  async function save() {
    setBusy(true); setError("");
    try {
      let request = draft.request;
      if (request.native) request = { ...request, native: { ...request.native, parameters: JSON.parse(nativeText) } };
      if (!draft.name.trim() || !request.sources.length) throw new Error("A name and at least one source are required.");
      for (const [key, label, min, max, scale] of numbers) {
        const value = draft[key];
        if (!Number.isInteger(value) || value < min * scale || value > max * scale) throw new Error(`${label} must be within ${min}–${max}, using whole seconds or records.`);
      }
      if (draft.max_records_per_run < (request.page_size ?? 25)) throw new Error("The record budget must fit one complete source page.");
      await onSave({ ...draft, name: draft.name.trim(), request });
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  }
  const numbers: Array<[Exclude<keyof SubscriptionSpec, "name" | "request" | "enabled">, string, number, number, number]> = [
    ["interval_seconds", "Check interval (minutes)", 1, 44640, 60], ["initial_days", "Initial lookback (days)", 1, 3650, 1], ["overlap_days", "Overlap (days)", 0, 30, 1], ["window_days", "Window size (days)", 1, 31, 1], ["max_pages_per_run", "Page attempts per check", 1, 1000, 1], ["max_records_per_run", "Records per check", 1, 100000, 1], ["run_timeout_ms", "Check time limit (seconds)", 1, 3600, 1000],
  ];
  return <SearchDialog title={title} onClose={onClose} footer={<div className="flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button><Button size="sm" disabled={busy} onClick={() => void save()}>{busy ? "Saving…" : "Save subscription"}</Button></div>}>
    <fieldset disabled={busy} className="space-y-3">
      <label className="block text-xs">Subscription name<Input aria-label="Subscription name" value={draft.name} maxLength={256} onChange={e => update("name", e.target.value)} /></label>
      <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={draft.enabled} onChange={e => update("enabled", e.target.checked)} />Enable automatic checks</label>
      {draft.request.native ? <label className="block text-xs">{draft.request.native.source} native query parameters<textarea aria-label="Subscription native query" rows={5} className="mt-1 w-full rounded border bg-background p-2 font-mono" value={nativeText} onChange={e => setNativeText(e.target.value)} /></label> : <>
        <label className="block text-xs">Query<Input aria-label="Subscription query" value={draft.request.query ?? ""} onChange={e => search({ query: e.target.value, expression: null })} /></label>
        {draft.request.expression && !draft.request.query && <p className="text-xs">The saved advanced expression is preserved. Enter a query above to replace it.</p>}
        <div className="flex flex-wrap gap-3 text-xs" aria-label="Subscription sources">{sources.map(source => <label key={source} className="flex items-center gap-1"><input type="checkbox" checked={draft.request.sources.includes(source)} onChange={e => search({ sources: e.target.checked ? [...draft.request.sources, source] : draft.request.sources.filter(s => s !== source) })} />{source}</label>)}</div>
      </>}
      <div className="grid grid-cols-2 gap-3 text-xs">
        <label>Publication year from<Input aria-label="Subscription year from" type="number" value={draft.request.filters?.year_from ?? ""} onChange={e => search({ filters: { ...draft.request.filters, year_from: e.target.value ? Number(e.target.value) : undefined } })} /></label>
        <label>Publication year to<Input aria-label="Subscription year to" type="number" value={draft.request.filters?.year_to ?? ""} onChange={e => search({ filters: { ...draft.request.filters, year_to: e.target.value ? Number(e.target.value) : undefined } })} /></label>
        {numbers.map(([key, label, min, max, scale]) => <label key={key}>{label}<Input aria-label={label} type="number" min={min} max={max} step={scale === 60 ? "any" : 1} value={(draft[key] as number) / scale} onChange={e => update(key, Number(e.target.value) * scale)} /></label>)}
      </div>
      <p className="text-xs text-muted-foreground">{strategies}</p>
      <p className="text-xs">Checks create local inbox notifications. Papers are imported only after your review. A record budget must fit a complete source page ({draft.request.page_size ?? 25} records).</p>
      <details className="text-xs"><summary>Saved search details</summary><pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words">{pretty(draft.request)}</pre></details>
    </fieldset>
    {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
  </SearchDialog>;
}
export function SubscribeSearchButton({ request }: { request: DiscoveryRequest }) {
  const [draft, setDraft] = useState<SubscriptionSpec | null>(null);
  const [saved, setSaved] = useState(false);
  return <>
    <Button size="xs" variant="ghost" onClick={() => { setSaved(false); setDraft(defaultSubscription(request)); }}>Subscribe to this search</Button>
    {saved && <span role="status" className="text-xs">Subscription saved. Review it in Subscriptions.</span>}
    {draft && <RuleEditor initial={draft} title="Subscribe to search" onClose={() => setDraft(null)} onSave={async spec => { await gatewayRequest("POST", api(), spec); setSaved(true); setDraft(null); }} />}
  </>;
}
const limitFields: Array<[keyof SubscriptionLimits, string, number]> = [
  ["max_rules", "Subscriptions", 1], ["max_seen", "Seen identities", 1], ["max_seen_bytes", "Seen identity data (MiB)", 1048576], ["max_inbox", "Inbox notifications", 1], ["max_inbox_bytes", "Inbox data (MiB)", 1048576], ["max_page_bytes", "Source page data (MiB)", 1048576], ["max_checkpoints", "Saved cursor checkpoints", 1], ["max_runs", "Execution history records", 1], ["max_run_bytes", "Single execution history (MiB)", 1048576], ["max_history_bytes", "All execution history (MiB)", 1048576],
];
function LimitsEditor({ limits, onSave, onClose }: { limits: SubscriptionLimits; onSave: (limits: SubscriptionLimits) => Promise<void>; onClose: () => void }) {
  const [draft, setDraft] = useState({ ...limits }); const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  return <SearchDialog title="Subscription storage limits" onClose={onClose} footer={<div className="flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button><Button size="sm" disabled={busy} onClick={() => { setBusy(true); setError(""); void onSave(draft).catch(e => setError(String(e))).finally(() => setBusy(false)); }}>Save storage limits</Button></div>}>
    <p className="mb-3 text-xs">Lowering a limit keeps existing data and may pause future checks. Changing available space or limits allows waiting subscriptions to retry.</p>
    <fieldset disabled={busy} className="grid grid-cols-2 gap-3">{limitFields.map(([key, label, scale]) => <label key={key} className="text-xs">{label}<Input aria-label={label} type="number" min={scale === 1 ? 1 : 1 / scale} step={scale === 1 ? 1 : "any"} value={draft[key] / scale} onChange={e => setDraft(old => ({ ...old, [key]: Math.round(Number(e.target.value) * scale) }))} /></label>)}</fieldset>
    {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
  </SearchDialog>;
}
function unique<T extends { id: string }>(head: T[], tail: T[]): T[] { const seen = new Set(head.map(row => row.id)); return [...head, ...tail.filter(row => !seen.has(row.id))]; }
export function Subscriptions({ active = true }: { active?: boolean }) {
  const [rules, setRules] = useState<SubscriptionRule[]>([]); const [inbox, setInbox] = useState<InboxItem[]>([]); const [nextInbox, setNextInbox] = useState<number | null>(null);
  const [limits, setLimits] = useState<SubscriptionLimits | null>(null); const [collections, setCollections] = useState<BibCollection[]>([]); const [collection, setCollection] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null); const selected = rules.find(r => r.id === selectedId);
  const [history, setHistory] = useState<RunSummary[]>([]); const [nextHistory, setNextHistory] = useState<number | null>(null); const [execution, setExecution] = useState<Run | null>(null);
  const [editor, setEditor] = useState<SubscriptionRule | null>(null); const [editLimits, setEditLimits] = useState(false);
  const [confirmation, setConfirmation] = useState<{ rule?: SubscriptionRule; item?: InboxItem; run?: Run } | null>(null);
  const [review, setReview] = useState<{ lookup: NotificationLookup; target: ExplorationImportTarget; label: string; key: string } | null>(null);
  const [error, setError] = useState(""); const [busy, setBusy] = useState(false); const [scheduler, setScheduler] = useState<{ running: boolean; active: number; last_error: string | null } | null>(null);
  const executionRef = useRef(execution); executionRef.current = execution;
  const loadSequence = useRef(0);
  const lifecycle = useRef<AbortController | null>(null); const pendingChecks = useRef(new Map<string, string>()); const selectedRef = useRef(selectedId); selectedRef.current = selectedId;
  const inboxMore = useRef(false); const historyMore = useRef(false); const mutation = useRef(false); const historyOwner = useRef<string | null>(null);
  async function fetchData(signal: AbortSignal, reset = false) {
    const sequence = ++loadSequence.current;
    const id = selectedRef.current;
    const [ruleData, inboxData, status] = await Promise.all([gatewayRequest("GET", api(), undefined, { signal }), gatewayRequest("GET", api("/inbox?limit=50"), undefined, { signal }), gatewayRequest("GET", api("/status"), undefined, { signal })]);
    if (signal.aborted || sequence !== loadSequence.current) return;
    setRules(ruleData.rules); setScheduler(status.scheduler);
    const ruleIds = new Set((ruleData.rules as SubscriptionRule[]).map(rule => rule.id));
    setInbox(old => (reset || inboxData.next === null ? inboxData.items as InboxItem[] : unique(inboxData.items as InboxItem[], old)).filter(item => ruleIds.has(item.rule_id)));
    if (id && !ruleIds.has(id)) { selectedRef.current = null; setSelectedId(null); setHistory([]); setExecution(null); }
    if (reset || !inboxMore.current) setNextInbox(inboxData.next);
    if (reset) inboxMore.current = false;
    if (id && ruleData.rules.some((r: SubscriptionRule) => r.id === id)) {
      const data = await gatewayRequest("GET", api(`${path(id)}/runs?limit=50`), undefined, { signal });
      if (signal.aborted || sequence !== loadSequence.current || selectedRef.current !== id) return;
      setHistory(old => reset || historyOwner.current !== id ? data.runs : unique(data.runs as RunSummary[], old));
      if (reset || !historyMore.current || historyOwner.current !== id) setNextHistory(data.next);
      historyOwner.current = id;
      const shown = executionRef.current;
      if (shown?.rule_id === id && shown.state === "running") {
        const latest = await gatewayRequest("GET", api(`/runs${path(shown.id)}`), undefined, { signal });
        if (!signal.aborted && sequence === loadSequence.current && executionRef.current?.id === shown.id) setExecution(latest.run);
      }
    }
  }
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController(); lifecycle.current = abort; let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { if (!mutation.current) await fetchData(abort.signal); } catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
      if (!abort.signal.aborted) timer = setTimeout(() => void poll(), 2500);
    };
    void poll();
    void Promise.all([gatewayRequest("GET", api("/limits"), undefined, { signal: abort.signal }), gatewayRequest("GET", "/bib/collections?view=summary", undefined, { signal: abort.signal })]).then(([a, b]) => { if (!abort.signal.aborted) { setLimits(a.limits); setCollections(b.collections ?? []); } }).catch(failure => { if (!abort.signal.aborted) setError(String(failure)); });
    return () => { clearTimeout(timer); abort.abort(); loadSequence.current++; lifecycle.current = null; setBusy(false); };
  }, [active]);
  async function act(work: (signal: AbortSignal) => Promise<void>) {
    const abort = lifecycle.current; if (!abort || mutation.current) return;
    loadSequence.current++; mutation.current = true; setBusy(true); setError("");
    try { await work(abort.signal); if (!abort.signal.aborted) await fetchData(abort.signal); }
    catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
    finally { mutation.current = false; if (!abort.signal.aborted) setBusy(false); }
  }
  function selectRule(id: string) { selectedRef.current = id; setSelectedId(id); setHistory([]); setNextHistory(null); setExecution(null); historyMore.current = false; if (lifecycle.current) void fetchData(lifecycle.current.signal).catch(failure => setError(String(failure))); }
  async function check(rule: SubscriptionRule, signal: AbortSignal) {
    const id = pendingChecks.current.get(rule.id) ?? newGatewayRequestId(); pendingChecks.current.set(rule.id, id);
    const result = await gatewayRequest("POST", api(`${path(rule.id)}/check`), { run_id: id }, { signal });
    pendingChecks.current.delete(rule.id); selectedRef.current = rule.id; setSelectedId(rule.id); setExecution(result.run);
  }
  function openReview(item: InboxItem, index: number) {
    const lookup = notificationLookup(item.records[index]); if (!lookup) return;
    const destination = collections.find(c => c.id === collection);
    if (collection && (!destination || destination.deleted)) { setError("The selected collection is unavailable. Choose an import destination before reviewing."); return; }
    setReview({ lookup, target: destination ? { collection_id: destination.id } : { library_id: 1 }, label: destination?.name ?? "Library", key: `${item.id}:${index}` });
  }
  async function remove(signal: AbortSignal) {
    if (confirmation?.rule) { const rule = confirmation.rule; await gatewayRequest("DELETE", api(`${path(rule.id)}?version=${rule.version}`), undefined, { signal }); setRules(old => old.filter(r => r.id !== rule.id)); if (selectedRef.current === rule.id) { selectedRef.current = null; setSelectedId(null); setHistory([]); setExecution(null); } }
    if (confirmation?.item) { const id = confirmation.item.id; await gatewayRequest("DELETE", api(`/inbox${path(id)}`), undefined, { signal }); setInbox(old => old.filter(i => i.id !== id)); }
    if (confirmation?.run) { const id = confirmation.run.id; await gatewayRequest("DELETE", api(`/runs${path(id)}`), undefined, { signal }); setHistory(old => old.filter(r => r.id !== id)); setExecution(null); }
    setConfirmation(null);
  }
  return <section aria-label="Literature subscriptions" className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3 text-sm">
    <header className="flex flex-wrap items-center justify-between gap-2"><h2 className="font-medium">Subscriptions</h2><div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => { historyMore.current = false; await fetchData(signal, true); })}>Refresh subscriptions</Button><Button size="sm" variant="outline" disabled={!limits} onClick={() => setEditLimits(true)}>Storage limits</Button></div></header>
    <p className="text-xs text-muted-foreground">Create subscriptions from an executed search in Discover. Checks continue while this view is closed. Only explicit pause or cancellation stops them.</p>
    {scheduler && <p className="text-xs">{scheduler.running ? `${scheduler.active} checks active` : "Background service stopped"}{scheduler.last_error && <span> · Last service error: {scheduler.last_error}</span>}</p>}
    {error && <p role="alert" className="text-xs text-destructive">{error}</p>}{busy && <p role="status" className="text-xs">Updating subscriptions…</p>}
    {editor && <RuleEditor key={editor.id} initial={editor.spec} title="Edit subscription" onClose={() => setEditor(null)} onSave={async spec => { await gatewayRequest("PUT", api(path(editor.id)), { version: editor.version, spec }); setEditor(null); if (lifecycle.current) await fetchData(lifecycle.current.signal); }} />}
    {editLimits && limits && <LimitsEditor limits={limits} onClose={() => setEditLimits(false)} onSave={async value => { const data = await gatewayRequest("PUT", api("/limits"), value); setLimits(data.limits); setEditLimits(false); if (lifecycle.current) await fetchData(lifecycle.current.signal); }} />}
    {review && <IdentityImportReview key={review.key} lookup={review.lookup.request} lookupLabel={review.lookup.label} accepts={review.lookup.accepts} target={review.target} targetLabel={review.label} onClose={() => setReview(null)} />}
    {confirmation && <SearchDialog title="Confirm subscription deletion" onClose={() => setConfirmation(null)}><p className="text-xs">{confirmation.rule ? `Delete “${confirmation.rule.spec.name}”, its inbox, seen identities and execution history?` : confirmation.item ? "Delete this notification? Seen identities remain remembered." : "Delete this execution history record?"} Canonical library papers are kept.</p><div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={() => setConfirmation(null)}>Keep record</Button><Button size="sm" disabled={busy} onClick={() => void act(remove)}>Confirm deletion</Button></div></SearchDialog>}
    <div className="grid gap-3 lg:grid-cols-[minmax(220px,35%)_minmax(0,1fr)]">
      <section aria-label="Subscription rules" className="min-w-0 space-y-2"><h3 className="font-medium">Saved subscriptions</h3>{!rules.length && <p className="text-xs">No subscriptions yet.</p>}
        {rules.map(rule => <article key={rule.id} className="space-y-2 rounded border p-2 text-xs"><button className="w-full break-words text-left font-medium" aria-pressed={selectedId === rule.id} onClick={() => selectRule(rule.id)}>{rule.spec.name}</button><p>{rule.spec.enabled ? "Automatic checks enabled" : "Paused"} · every {rule.spec.interval_seconds / 60} min</p><p>{rule.spec.request.sources.join(", ")}</p><p>Last complete check: {date(rule.last_success_ms)}</p><p>Next scheduled check: {rule.spec.enabled ? date(rule.next_due_ms) : "Paused"}</p>{rule.resource_wait && <p role="status" className="text-amber-700">Waiting for storage: {rule.resource_wait.reason}. Free space or change limits to retry.</p>}
          <div className="flex flex-wrap gap-1"><Button size="xs" variant="outline" disabled={busy || Boolean(rule.resource_wait)} onClick={() => void act(signal => check(rule, signal))}>{pendingChecks.current.has(rule.id) ? "Retry check acknowledgement" : "Check now"}</Button><Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("POST", api(`${path(rule.id)}/enabled`), { version: rule.version, enabled: !rule.spec.enabled }, { signal }); })}>{rule.spec.enabled ? "Pause" : "Resume"}</Button><Button size="xs" variant="ghost" disabled={busy} onClick={() => setEditor(structuredClone(rule))}>Edit rule</Button><Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirmation({ rule: structuredClone(rule) })}>Delete rule</Button></div>
        </article>)}
      </section>
      <section aria-label="Subscription execution history" className="min-w-0 space-y-2"><h3 className="font-medium">Execution history{selected ? ` · ${selected.spec.name}` : ""}</h3>{!selected && <p className="text-xs">Select a subscription to inspect its checks and coverage.</p>}
        {selected && <><details className="text-xs"><summary>Current source coverage</summary><p className="my-2 text-muted-foreground">{strategies}</p>{selected.sources.map(source => <div key={source.source} className="mb-2 rounded border p-2"><p className="font-medium">{source.source}</p><p>{source.scan?.strategy === "bounded_replay" ? "Bounded replay; no update watermark" : `Completed update watermark: ${date(source.watermark_ms)}`}</p><p>{source.complete ? "Current window exhausted" : "Window has more results"} · Last checked: {date(source.last_checked_ms)}</p>{source.last_error && <p className="text-destructive">{source.last_error.message}</p>}{source.retry_at_ms && <p>Do not retry before: {date(source.retry_at_ms)}</p>}<pre className="max-h-36 overflow-auto whitespace-pre-wrap break-words">{pretty({ window: source.window, plan: source.plan })}</pre></div>)}</details>
          {!history.length && <p className="text-xs">No execution history loaded.</p>}{history.map(run => <div key={run.id} className="flex flex-wrap items-center gap-2 rounded border p-2 text-xs"><button className="flex-1 text-left" onClick={() => void act(async signal => { const data = await gatewayRequest("GET", api(`/runs${path(run.id)}`), undefined, { signal }); setExecution(data.run); })}>{date(run.started_ms)} · {run.state} · {run.records} committed records · {run.new_notifications} new notifications</button>{run.state === "running" && <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => { const data = await gatewayRequest("POST", api(`/runs${path(run.id)}/cancel`), {}, { signal }); setExecution(data.run); })}>Cancel check</Button>}</div>)}
          {nextHistory !== null && <Button size="xs" variant="outline" disabled={busy} onClick={() => void act(async signal => { const data = await gatewayRequest("GET", api(`${path(selected.id)}/runs?before=${nextHistory}&limit=50`), undefined, { signal }); historyMore.current = true; setHistory(old => unique(old, data.runs)); setNextHistory(data.next); })}>Load more checks</Button>}
        </>}
        {execution && <article aria-label="Execution details" className="space-y-2 rounded border p-2 text-xs"><h4 className="font-medium">Check · {execution.state}</h4><p>{execution.pages_started} page attempts · {execution.pages_committed} pages committed · {execution.records} records committed · {execution.new_notifications} new notifications</p>{execution.reason && <p>{execution.reason}</p>}{execution.state !== "complete" && <p>This check is incomplete; it does not mean there were no new matches.</p>}{execution.sources.map(source => <p key={source.source}>{source.source}: {source.done ? "frozen target completed" : "incomplete"}{source.error && ` · ${source.error.message}`}</p>)}<details><summary>Saved execution plan</summary><pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words">{pretty(execution)}</pre></details>{execution.state !== "running" && <Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirmation({ run: execution })}>Delete execution record</Button>}</article>}
      </section>
    </div>
    <section aria-label="Subscription inbox" className="space-y-2"><div className="flex flex-wrap items-center justify-between gap-2"><h3 className="font-medium">New matches</h3><label className="text-xs">Import destination <select aria-label="Subscription import destination" className="rounded border bg-background p-1" value={collection} onChange={e => setCollection(e.target.value)}><option value="">Library</option>{collection && !collections.some(c => c.id === collection && !c.deleted) && <option value={collection}>Unavailable collection</option>}{collections.filter(c => !c.deleted).map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label></div>
      {!inbox.length && <p className="text-xs">No notifications loaded. Check execution history for failures or incomplete scans.</p>}
      {inbox.map(item => <article key={item.id} className="space-y-2 rounded border p-2 text-xs"><p>{item.read ? "Read" : "Unread"} · {date(item.first_matched_ms)} · {rules.find(r => r.id === item.rule_id)?.spec.name ?? "Subscription"} · rule version {item.rule_version}</p>{item.related_groups.length > 0 && <p className="text-amber-700">Related identities need review; these versions were not automatically merged.</p>}{item.records.map((record, index) => <div key={`${record.source}:${record.article.id}:${index}`} className="rounded border p-2"><p className="break-words font-medium">{record.article.title}</p><p>{record.source}{record.article.year && ` · ${record.article.year}`}</p><p className="break-words text-muted-foreground">{record.article.identifiers?.map(id => `${id.kind}: ${id.value}`).join(" · ")}</p>{notificationLookup(record) ? <Button size="xs" variant="outline" onClick={() => openReview(item, index)}>Review notification import</Button> : <p>No reliable supported identifier is available for a fresh lookup.</p>}</div>)}<div className="flex gap-2"><Button size="xs" variant="ghost" disabled={busy} onClick={() => void act(async signal => { await gatewayRequest("PUT", api(`/inbox${path(item.id)}`), { read: !item.read }, { signal }); setInbox(old => old.map(row => row.id === item.id ? { ...row, read: !row.read } : row)); })}>{item.read ? "Mark unread" : "Mark read"}</Button><Button size="xs" variant="ghost" disabled={busy} onClick={() => setConfirmation({ item })}>Delete notification</Button></div></article>)}
      {nextInbox !== null && <Button size="sm" variant="outline" disabled={busy} onClick={() => void act(async signal => { const data = await gatewayRequest("GET", api(`/inbox?before=${nextInbox}&limit=50`), undefined, { signal }); inboxMore.current = true; setInbox(old => unique(old, data.items)); setNextInbox(data.next); })}>Load more notifications</Button>}
    </section>
  </section>;
}
