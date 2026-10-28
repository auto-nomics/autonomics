import { useCallback, useEffect, useRef, useState } from "react";
import { gatewayRequest, newGatewayRequestId } from "@/gateway";
import type { DiscoveryCandidate, DiscoveryOrder, DiscoveryRequest, DiscoveryResultPage, DiscoverySession } from "./discovery-model";

type Active = { id: string; abort: AbortController };
const path = (id: string) => `/bib/search/sessions/${encodeURIComponent(id)}`;

/** Session lifetime and result-page lifetime are separate. Incoming batches
 * update progress immediately, while an explicit refresh preserves row order,
 * focus and the fingerprints of the user's import selections. */
export function useDiscovery() {
  const active = useRef<Active | null>(null);
  const resultsAbort = useRef<AbortController | null>(null);
  const pollAbort = useRef<AbortController | null>(null);
  const resultSequence = useRef(0);
  const displayed = useRef<DiscoveryCandidate[]>([]);
  const [session, setSession] = useState<DiscoverySession | null>(null);
  const [candidates, setCandidates] = useState<DiscoveryCandidate[]>([]);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [obtainedCandidates, setObtainedCandidates] = useState(0);
  const [updatesPending, setUpdatesPending] = useState(false);
  const [creating, setCreating] = useState(false);
  const [loadingResults, setLoadingResults] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<{ source: string; order: DiscoveryOrder }>({ source: "", order: "first_seen" });
  const filterRef = useRef(filter); filterRef.current = filter;

  const close = useCallback(() => {
    const previous = active.current;
    active.current = null;
    previous?.abort.abort(); resultsAbort.current?.abort(); pollAbort.current?.abort();
    // This UUID is known before POST starts, including an aborted create.
    if (previous) void gatewayRequest("DELETE", path(previous.id)).catch(() => {});
  }, []);
  useEffect(() => close, [close]);

  const loadPage = useCallback(async (current: Active, append = false, chosen = filterRef.current) => {
    resultsAbort.current?.abort();
    const abort = new AbortController(); resultsAbort.current = abort;
    const cancelled = () => abort.abort();
    current.abort.signal.addEventListener("abort", cancelled, { once: true });
    if (current.abort.signal.aborted) abort.abort();
    setLoadingResults(true);
    try {
      const params = new URLSearchParams({ limit: "100", offset: String(append ? displayed.current.length : 0), order: chosen.order });
      if (chosen.source) params.set("source", chosen.source);
      if (append) params.set("sequence", String(resultSequence.current));
      const page: DiscoveryResultPage = await gatewayRequest("GET", `${path(current.id)}/results?${params}`, undefined, { signal: abort.signal });
      if (active.current !== current || abort.signal.aborted) return;
      const next = append ? [...displayed.current, ...page.candidates] : page.candidates;
      displayed.current = next; resultSequence.current = page.sequence;
      setCandidates(next); setNextOffset(page.next_offset); setObtainedCandidates(page.obtained_candidates); setUpdatesPending(false); setError("");
    } catch (failure) {
      if (active.current === current && !abort.signal.aborted) { setError(`${String(failure)}. Refresh results before continuing.`); setUpdatesPending(true); }
    } finally {
      current.abort.signal.removeEventListener("abort", cancelled);
      if (resultsAbort.current === abort) setLoadingResults(false);
    }
  }, []);

  const watch = useCallback(async (current: Active, after: number) => {
    pollAbort.current?.abort();
    const abort = new AbortController(); pollAbort.current = abort;
    const cancelled = () => abort.abort();
    current.abort.signal.addEventListener("abort", cancelled, { once: true });
    if (current.abort.signal.aborted) abort.abort();
    try {
      while (!abort.signal.aborted && active.current === current) {
        const events: { sequence: number; reset_required: boolean; events: Array<{ type: string; sequence: number }> } = await gatewayRequest("GET", `${path(current.id)}/events?after=${after}&wait_ms=20000`, undefined, { signal: abort.signal });
        const response: { session: DiscoverySession } = await gatewayRequest("GET", path(current.id), undefined, { signal: abort.signal });
        if (active.current !== current || abort.signal.aborted) return;
        setSession(response.session);
        // Reconnecting after an action replays retained history. Only a batch
        // newer than the displayed snapshot represents new result content;
        // a newer source_started event must not promote an old batch.
        const changed = events.reset_required || events.events.some(event => event.type === "batch" && event.sequence > resultSequence.current);
        if (!displayed.current.length && response.session.merged_candidates > 0) await loadPage(current);
        else if (changed && events.sequence > resultSequence.current) setUpdatesPending(true);
        // Advance only through delivered events, even if the separate snapshot
        // already includes a later source completion.
        after = events.sequence;
        if (response.session.phase !== "running" && response.session.sequence <= after) break;
      }
    } catch (failure) {
      if (!abort.signal.aborted && active.current === current) setError(`Progress connection interrupted: ${String(failure)}. Resume progress to reconnect.`);
    } finally { current.abort.signal.removeEventListener("abort", cancelled); }
  }, [loadPage]);

  const start = useCallback(async (request: DiscoveryRequest) => {
    close(); setCreating(true); setError(""); setSession(null);
    // A source filter belongs to the available sources of this request.
    // Keeping a removed source would hide valid results behind a stale filter.
    if (filterRef.current.source && !request.sources.includes(filterRef.current.source)) {
      const next = { ...filterRef.current, source: "" };
      filterRef.current = next; setFilter(next);
    }
    displayed.current = []; resultSequence.current = 0;
    setCandidates([]); setNextOffset(null); setObtainedCandidates(0); setUpdatesPending(false);
    const current: Active = { id: newGatewayRequestId(), abort: new AbortController() };
    active.current = current;
    try {
      const response: { session: DiscoverySession } = await gatewayRequest("POST", "/bib/search/sessions", request, { signal: current.abort.signal, requestId: current.id });
      if (active.current !== current) return;
      setSession(response.session);
      await loadPage(current);
      void watch(current, response.session.sequence);
    } catch (failure) {
      if (!current.abort.signal.aborted && active.current === current) {
        setError(String(failure));
        void gatewayRequest("DELETE", path(current.id)).catch(() => {});
      }
    } finally { if (active.current === current) setCreating(false); }
  }, [close, loadPage, watch]);

  const sourceAction = useCallback(async (action: "cancel" | "next" | "retry", source?: string) => {
    const current = active.current; if (!current) return;
    try {
      const response: { session: DiscoverySession } = await gatewayRequest("POST", action === "cancel" ? `${path(current.id)}/cancel` : `${path(current.id)}/sources/${encodeURIComponent(source!)}/${action}`, action === "cancel" ? { source: source ?? null } : undefined, { signal: current.abort.signal });
      if (active.current !== current) return;
      setSession(response.session); setError("");
      void watch(current, 0);
    } catch (failure) { if (!current.abort.signal.aborted && active.current === current) setError(String(failure)); }
  }, [watch]);
  const refresh = useCallback(() => { const current = active.current; if (current) void loadPage(current); }, [loadPage]);
  const more = useCallback(() => { const current = active.current; if (current) void loadPage(current, true); }, [loadPage]);
  const changeFilter = useCallback((source: string, order: DiscoveryOrder) => {
    const next = { source, order }; filterRef.current = next; setFilter(next);
    const current = active.current; if (current) void loadPage(current, false, next);
  }, [loadPage]);
  const resume = useCallback(() => { const current = active.current; if (current) { setError(""); void watch(current, 0); } }, [watch]);
  const cancelCreation = useCallback(() => { close(); setCreating(false); setSession(null); }, [close]);
  return { session, candidates, nextOffset, obtainedCandidates, updatesPending, creating, loadingResults, error, filter, start, sourceAction, refresh, more, changeFilter, resume, cancelCreation };
}
